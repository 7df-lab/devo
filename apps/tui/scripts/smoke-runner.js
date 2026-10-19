/** Product TTY smoke: assert outcomes without changing product behavior. */
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { heartbeatDelivered, rpcSucceeded, turnOutcome } from "./smoke-outcomes.js";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const scenario = process.argv[2] || "T1";
const scenarios = ["T1", "T2", "T3", "goal", "bash", "agents", "schedule", "heartbeat"];
const tmux = process.env.DEVO_TMUX_BIN || (process.platform === "win32" ? "psmux" : "tmux");
const binary = resolve(process.env.DEVO_BIN || process.env.DEVO_SERVER_BIN
  || join(repo, "target/debug", process.platform === "win32" ? "devo.exe" : "devo"));
const session = process.env.DEVO_TUI_TEST_SESSION || `devo-smoke-${process.pid}-${Date.now()}`;
const timeout = Number(process.env.DEVO_TUI_TEST_TIMEOUT_MS || 90000);

function mux(args, required = true) {
  const result = spawnSync(tmux, args, { encoding: "utf8", windowsHide: true, timeout: 10000 });
  if (required && (result.error || result.status !== 0)) {
    throw new Error(`${tmux} ${args[0]} failed: ${result.error?.message || result.stderr || result.status}`);
  }
  return result;
}

async function run() {
  if (!scenarios.includes(scenario)) throw new Error(`Unknown scenario: ${scenario}`);
  if (!existsSync(binary)) throw new Error(`Build devo first: cargo build -p devo-cli --bin devo (${binary})`);
  if (!Number.isFinite(timeout) || timeout <= 0) throw new Error("Invalid DEVO_TUI_TEST_TIMEOUT_MS");
  if (mux(["has-session", "-t", session], false).status === 0) throw new Error(`Session already exists: ${session}`);

  const home = mkdtempSync(join(tmpdir(), "devo-tui-smoke-"));
  const trace = join(home, "protocol.ndjsonl");
  const sourceHome = process.env.DEVO_HOME || join(homedir(), ".devo");
  let ownedSession = false;
  let pane = "";
  try {
    for (const name of ["auth.json", "providers.json", "config.toml"]) {
      if (existsSync(join(sourceHome, name))) copyFileSync(join(sourceHome, name), join(home, name));
    }
    const settingsPath = join(sourceHome, "settings.json");
    const settings = existsSync(settingsPath) ? JSON.parse(readFileSync(settingsPath, "utf8")) : {};
    writeFileSync(join(home, "settings.json"), JSON.stringify({ ...settings, onboardingShown: true }));
    mux(["new-session", "-d", "-s", session, "-c", repo, "-x", "100", "-y", "30",
      "-e", `DEVO_HOME=${home}`, "-e", `DEVO_CODING_AGENT_DIR=${home}`,
      "-e", `PRIME_AGENT_CODING_AGENT_DIR=${home}`, "-e", "DEVO_PROTOCOL_TRACE=1",
      "-e", `DEVO_PROTOCOL_TRACE_FILE=${trace}`, "--", binary]);
    ownedSession = true;

    function records() {
      if (!existsSync(trace)) return [];
      // Ignore a final incomplete frame while the trace writer appends it.
      return readFileSync(trace, "utf8").split("\n").slice(0, -1).filter(Boolean).map((line) => JSON.parse(line));
    }

    async function waitFor(label, predicate) {
      const deadline = Date.now() + timeout;
      do {
        pane = mux(["capture-pane", "-t", session, "-p", "-S", "-300"]).stdout;
        if (predicate(records(), pane)) return;
        await sleep(300);
      } while (Date.now() < deadline);
      throw new Error(`Timed out waiting for ${label}`);
    }

    async function send(text) {
      mux(["send-keys", "-t", session, "-l", text]);
      await sleep(150);
      mux(["send-keys", "-t", session, "Enter"]);
    }

    await waitFor("ready chat chrome", (events, text) => rpcSucceeded(events, "session/new") && text.includes("← manage"));
    console.log("=== boot ===\n" + pane);
    switch (scenario) {
      case "T1": break;
      case "T2":
        await send("Say hello in one short English sentence.");
        await waitFor("completed turn with user and assistant/tool activity", (events) => turnOutcome(events, "completed"));
        break;
      case "T3":
        await send("Use ipython to await asyncio.sleep(30), then print a short greeting.");
        await waitFor("active turn", (events) => events.some((r) => r.method === "turn/started"));
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("interrupted turn", (events) => turnOutcome(events, "interrupted"));
        break;
      case "goal":
        await send("/goal smoke objective");
        await waitFor("goal creation", (events) => rpcSucceeded(events, "session/goal/set"));
        await send("/goal pause");
        await waitFor("goal pause", (events) => rpcSucceeded(events, "session/goal/pause"));
        break;
      case "bash":
        await send("!echo smoke-bash");
        await waitFor("shell output", (_events, text) => /^\s*(?:[›│╰─]+\s*)?smoke-bash\s*$/m.test(text));
        break;
      case "agents":
        mux(["send-keys", "-t", session, "Left"]);
        await waitFor("agent manager", (_events, text) => text.includes("Search sessions") && text.includes("navigate"));
        break;
      case "schedule":
        // An explicit argument avoids the bare slash-command completion step.
        await send("/heartbeat status");
        await waitFor("heartbeat status", (_events, text) => /No active heartbeat|Heartbeat(?:s)?\s+(?:manager|scheduled)/i.test(text));
        break;
      case "heartbeat": {
        const marker = `hb-tui-fire-${Date.now()}`;
        await send(`/heartbeat every 1s ${marker}`);
        // A slash-command echo also contains the marker. Require a delivered
        // Native user item rather than matching the echoed command.
        await waitFor("delivered heartbeat", (events) => heartbeatDelivered(events, marker));
        await send("/heartbeat clear");
        await waitFor("heartbeat clear", (_events, text) => /Heartbeat cleared/i.test(text));
        break;
      }
    }
    console.log(`=== ${scenario} outcome ===\n${pane}\n${scenario} passed (outcome verified)`);
  } catch (error) {
    console.error(`=== last pane ===\n${pane}`);
    throw error;
  } finally {
    if (ownedSession) mux(["kill-session", "-t", session], false);
    // The target is our own mkdtemp child, never the source credential folder.
    if (dirname(home) !== resolve(tmpdir())) throw new Error("Unexpected smoke home location");
    rmSync(home, { recursive: true, force: true });
  }
}

run().catch((error) => { console.error(error.message); process.exitCode = 1; });
