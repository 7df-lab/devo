/** Baseline observations through the real Rust CLI -> Node TUI -> Native server. */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { cpus, release, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { createOwnedSmokeTmux } from "./owned-smoke-tmux.mjs";
import { parseTrace, positiveInteger, sessionResult, summarize, waitForObservation } from "./tui-latency-support.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const binary = resolve(process.env.DEVO_BIN || join(repo, "target/debug", process.platform === "win32" ? "devo.exe" : "devo"));
const tmux = process.env.DEVO_TMUX_BIN || (process.platform === "win32" ? "psmux" : "tmux");
const samples = positiveInteger(process.env.DEVO_BENCH_SAMPLES || 5, "DEVO_BENCH_SAMPLES");
const pollMs = positiveInteger(process.env.DEVO_BENCH_POLL_MS || 25, "DEVO_BENCH_POLL_MS");
const timeoutMs = positiveInteger(process.env.DEVO_BENCH_TIMEOUT_MS || 60000, "DEVO_BENCH_TIMEOUT_MS");
const output = resolve(process.env.DEVO_BENCH_OUTPUT || "tui-latency.json");
const root = mkdtempSync(join(tmpdir(), "devo-tui-latency-"));
const server = createOwnedSmokeTmux({ directory: root, executable: tmux });
const { mux } = server;
const report = {
  schema_version: 1, status: "incomplete", timestamp: new Date().toISOString(),
  scenario: "offline-empty-sessions", terminal: { columns: 100, rows: 30 },
  poll_ms: pollMs, timeout_ms: timeoutMs, requested_samples: samples,
  environment: { platform: process.platform, arch: process.arch, os_release: release(), node: process.version,
    cpu_model: cpus()[0]?.model, logical_cpus: cpus().length },
  source_revision: spawnSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8" }).stdout?.trim(),
  source_dirty: Boolean(spawnSync("git", ["status", "--porcelain"], { cwd: repo, encoding: "utf8" }).stdout?.trim()),
  samples: [],
};
try {
  assert.ok(existsSync(binary), `Build this checkout first: cargo build -p devo-cli --bin devo (${binary})`);
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(binary)) hash.update(chunk);
  report.binary_sha256 = hash.digest("hex");
  report.tmux_version = spawnSync(tmux, ["-V"], { encoding: "utf8" }).stdout?.trim();
  server.start();
  for (let index = 0; index < samples; index++) {
    const home = mkdtempSync(join(root, "home-"));
    const cwd = mkdtempSync(join(root, "workspace-"));
    // No personal settings, credentials, history, project hooks or extensions.
    writeFileSync(join(home, "settings.json"), JSON.stringify({ onboardingShown: true }));
    writeFileSync(join(home, "config.toml"), "[catalog]\noffline = true\n[updates]\ncheck_on_startup = false\n");
    const trace = join(home, "trace.ndjsonl");
    const session = `sample-${index}`;
    const records = () => existsSync(trace) ? parseTrace(readFileSync(trace, "utf8")) : [];
    const observe = () => ({ text: mux(["capture-pane", "-t", session, "-p"]).stdout, records: records() });
    const wait = (label, predicate) => waitForObservation({ label, predicate, observe, now: () => performance.now(), sleep, timeoutMs, pollMs });
    const send = text => mux(["send-keys", "-t", session, "-l", text]);
    const keys = (...keys) => mux(["send-keys", "-t", session, ...keys]);
    const clear = async marker => {
      keys("C-u");
      await wait("draft cleared", state => !state.text.includes(marker));
    };
    const probe = async label => {
      const marker = `BENCH_${index}_${label}`;
      send(marker); // Never submit probes or start a model turn.
      await wait(`${label} editable`, state => state.text.includes(marker));
      return marker;
    };
    const sample = { index };
    const launch = performance.now();
    mux(["new-session", "-d", "-s", session, "-c", cwd, "-x", "100", "-y", "30",
      "-e", "TERM=xterm-256color", "-e", `DEVO_HOME=${home}`, "-e", `DEVO_CODING_AGENT_DIR=${home}`,
      "-e", `PRIME_AGENT_CODING_AGENT_DIR=${home}`, "-e", "DEVO_PROTOCOL_TRACE=1", "-e", `DEVO_PROTOCOL_TRACE_FILE=${trace}`,
      "--", binary]);
    await wait("first trust screen", state => state.text.includes("Trust this workspace?") && state.text.includes("› No, exit"));
    sample.first_trust_screen_ms = performance.now() - launch;
    const accepted = performance.now();
    keys("Down", "Enter"); // Only our empty, newly-created workspace.
    const initial = await wait("initial session and chrome", state => sessionResult(state.records, "session/new")
      && state.text.includes("Subagents") && state.text.includes("Scheduled") && state.text.includes("Shells"));
    const original = sessionResult(initial.records, "session/new");
    let marker = await probe("START");
    sample.accepted_to_editable_ms = performance.now() - accepted;
    await clear(marker);
    const typing = performance.now();
    marker = await probe("TYPE");
    sample.input_echo_ms = performance.now() - typing;
    await clear(marker);

    let offset = records().length;
    send("/new");
    await wait("new command typed", state => state.text.includes("/new"));
    const creating = performance.now();
    keys("Enter");
    const created = await wait("new session rendered", state => sessionResult(state.records.slice(offset), "session/new")
      && state.text.includes("New session started"));
    assert.notEqual(sessionResult(created.records.slice(offset), "session/new"), original);
    marker = await probe("NEW");
    sample.new_session_editable_ms = performance.now() - creating;
    await clear(marker);

    offset = records().length;
    send(`/resume ${original}`);
    await wait("resume command typed", state => state.text.includes(`/resume ${original}`));
    const resuming = performance.now();
    keys("Enter");
    await wait("original session restored", state => sessionResult(state.records.slice(offset), "session/resume", original)
      && state.text.includes("Resumed session"));
    marker = await probe("RESUME");
    sample.resume_empty_editable_ms = performance.now() - resuming;
    await clear(marker);
    assert.ok(!records().some(record => record.direction === "tui-to-server" && record.method === "turn/start"), "benchmark must not start inference");
    mux(["kill-session", "-t", session]);
    report.samples.push(sample);
    console.log(`Completed PTY baseline sample ${index + 1}/${samples}`);
  }
  report.summary_ms = summarize(report.samples);
  report.status = "complete";
} catch (error) {
  report.status = "failed";
  report.error = error instanceof Error ? error.message : String(error);
  process.exitCode = 1;
} finally {
  server.close();
  rmSync(root, { recursive: true, force: true });
  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
}
console.log(JSON.stringify(report, null, 2));
