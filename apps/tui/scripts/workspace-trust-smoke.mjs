/** Real product TTY admission: no trust or server activation on cancellation. */
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { createOwnedSmokeTmux } from "./owned-smoke-tmux.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const binary = resolve(process.env.DEVO_BIN || join(repo, "target/debug", process.platform === "win32" ? "devo.exe" : "devo"));
const tmux = process.env.DEVO_TMUX_BIN || (process.platform === "win32" ? "psmux" : "tmux");
const root = mkdtempSync(join(tmpdir(), "devo-trust-smoke-"));
const server = createOwnedSmokeTmux({ directory: root, executable: tmux });
const { mux } = server;
async function waitFor(label, predicate) {
  const deadline = Date.now() + 60000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await sleep(100);
  }
  throw new Error(`Timed out: ${label}`);
}
const records = path => existsSync(path) ? readFileSync(path, "utf8").split("\n").slice(0, -1).filter(Boolean).map(JSON.parse) : [];
// This assertion uses the same trace contract as the other product smoke tests.
const { rpcSucceeded } = await import("./smoke-outcomes.js");
function launch(label, home, width) {
  const session = `devo-trust-${process.pid}-${label}`;
  const trace = join(home, `${label}.ndjsonl`);
  mux(["new-session", "-d", "-s", session, "-c", repo, "-x", String(width), "-y", "30",
    "-e", `DEVO_HOME=${home}`, "-e", `DEVO_CODING_AGENT_DIR=${home}`, "-e", `PRIME_AGENT_CODING_AGENT_DIR=${home}`,
    "-e", "DEVO_PROTOCOL_TRACE=1", "-e", `DEVO_PROTOCOL_TRACE_FILE=${trace}`, "--", binary]);
  return { session, trace, pane: () => mux(["capture-pane", "-t", session, "-p"]).stdout };
}
function stop(session) {
  mux(["kill-session", "-t", session]);
}
try {
  assert.ok(existsSync(binary), "build devo-cli before this smoke");
  server.start();
  for (const [label, keys] of [
    ["default", ["Enter"]], ["escape", ["Down", "Escape"]],
    ["ctrl-c", ["Down", "C-c"]], ["ctrl-d", ["Down", "C-d"]], ["typed-yes", ["y", "e", "s", "Enter"]],
  ]) {
    const home = mkdtempSync(join(root, `${label}-`));
    const { session, trace, pane } = launch(label, home, 100);
    await waitFor("default No", () => pane().includes("› No, exit"));
    assert.match(pane(), /Yes, I trust this folder/);
    assert.equal(records(trace).length, 0, "no server RPC before consent");
    mux(["send-keys", "-t", session, ...keys]);
    await waitFor("declined process exit", () => mux(["has-session", "-t", session], false).status !== 0);
    assert.ok(!existsSync(join(home, "workspace-trust")), "decline cannot persist trust");
    assert.equal(records(trace).length, 0, "decline cannot activate a session");
  }
  const home = mkdtempSync(join(root, "accept-"));
  writeFileSync(join(home, "settings.json"), JSON.stringify({ onboardingShown: true }));
  const first = launch("accept", home, 100);
  await waitFor("accept prompt", () => first.pane().includes("› No, exit"));
  mux(["resize-window", "-t", first.session, "-x", "40", "-y", "30"]);
  await waitFor("narrow prompt", () => first.pane().includes("Yes, I trust this folder"));
  mux(["send-keys", "-t", first.session, "Down"]);
  await waitFor("Yes selected", () => first.pane().includes("› Yes, I trust this folder"));
  mux(["send-keys", "-t", first.session, "Up"]);
  await waitFor("No selected again", () => first.pane().includes("› No, exit"));
  mux(["send-keys", "-t", first.session, "Down", "Enter"]);
  await waitFor("session activated", () => rpcSucceeded(records(first.trace), "session/new"));
  assert.equal(readdirSync(join(home, "workspace-trust")).filter(name => name.endsWith(".json")).length, 1);
  stop(first.session);
  const second = launch("remembered", home, 100);
  await waitFor("remembered trust starts without input", () => rpcSucceeded(records(second.trace), "session/new"));
  assert.ok(!second.pane().includes("Trust this workspace?"));
  stop(second.session);
  console.log("Workspace trust real TTY: default No, cancellation, arrows, resize, acceptance, remembered trust passed");
} finally {
  server.close();
  rmSync(root, { recursive: true, force: true });
}
