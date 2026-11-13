/** Product TTY smoke: assert outcomes without changing product behavior. */
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";
import { scheduledDelivered, rpcSucceeded, turnOutcome } from "./smoke-outcomes.js";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const scenario = process.argv[2] || "T1";
const scenarios = ["T1", "T2", "T3", "goal", "bash", "agents", "schedule", "heartbeat", "activity", "effort", "theme", "fork", "clipboard"];
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
      if (!(["effort", "theme"].includes(scenario)) && existsSync(join(sourceHome, name))) copyFileSync(join(sourceHome, name), join(home, name));
    }
    if (["effort", "theme"].includes(scenario)) {
      // Self-contained metadata fixture: selectors require no model request or credentials.
      writeFileSync(join(home, "providers.json"), JSON.stringify({
        model: "smoke/reasoner", provider: { smoke: {
          name: "Smoke", enabled: true, base_url: "http://127.0.0.1:9/v1",
          wire_api: "openai_chat_completions", models: { reasoner: {
            origin: "user", name: "Smoke reasoner", context_window: 32768,
            max_tokens: 4096, input_modalities: ["text"],
            reasoning_capability: { levels: ["low", "medium", "high"] },
            default_reasoning_effort: "medium",
          } },
        } },
      }));
    }
    const settingsPath = join(sourceHome, "settings.json");
    const settings = !(["effort", "theme"].includes(scenario)) && existsSync(settingsPath) ? JSON.parse(readFileSync(settingsPath, "utf8")) : {};
    writeFileSync(join(home, "settings.json"), JSON.stringify({ ...settings, ...(scenario === "theme" ? { theme: "dark" } : {}), onboardingShown: true }));
    if (scenario === "clipboard") {
      if (process.platform === "win32") throw new Error("clipboard substitute smoke currently requires Unix tmux");
      const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
      const loader = pathToFileURL(join(repo, "apps/tui/node_modules/tsx/dist/loader.mjs"));
      const preload = pathToFileURL(join(repo, "apps/tui/scripts/clipboard-smoke-preload.mjs"));
      // NODE_OPTIONS disallows the test-module-mocks flag. Scope a node wrapper
      // to this tmux child instead. Load tsx directly so its CLI subprocess
      // cannot drop our explicit test flags; CLI/server/TUI sources stay unchanged.
      writeFileSync(join(home, "node"), `#!/bin/sh\ncase "$1" in */tsx/dist/cli.mjs) shift ;; esac\nexec ${quote(process.execPath)} --experimental-test-module-mocks --import=${quote(String(loader))} --import=${quote(String(preload))} "$@"\n`, { mode: 0o700 });
    }
    mux(["new-session", "-d", "-s", session, "-c", repo, "-x", "100", "-y", "30",
      "-e", `DEVO_HOME=${home}`, "-e", `DEVO_CODING_AGENT_DIR=${home}`,
      "-e", `PRIME_AGENT_CODING_AGENT_DIR=${home}`, "-e", "DEVO_PROTOCOL_TRACE=1",
      "-e", `DEVO_PROTOCOL_TRACE_FILE=${trace}`,
      ...(scenario === "clipboard" ? [
        "-e", `DEVO_CLIPBOARD_SMOKE_DIR=${home}`,
        "-e", `PATH=${home}:${process.env.PATH}`,
      ] : []), "--", ...(scenario === "clipboard" ? ["env", `PATH=${home}:${process.env.PATH}`] : []), binary]);
    ownedSession = true;
    if (scenario === "clipboard") {
      // A multi-argument tmux command executes directly, avoiding login-shell
      // startup files replacing PATH and silently bypassing the node wrapper.
      const deadline = Date.now() + 15000;
      while (!existsSync(join(home, "preload.ready"))) {
        if (Date.now() > deadline) throw new Error("Clipboard smoke node preload did not start");
        await sleep(100);
      }
    }

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

    // The isolated test home has no saved approvals; admit only this test repo.
    await waitFor("workspace trust", (_events, text) => text.includes("Trust this workspace?"));
    await send("yes");
    await waitFor("ready chat chrome", (events, text) => rpcSucceeded(events, "session/new") && text.includes("Subagents") && text.includes("Scheduled") && text.includes("Shells"));
    console.log("=== boot ===\n" + pane);
    switch (scenario) {
      case "T1": break;
      case "clipboard": {
        const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
        let requestId = 0;
        async function begin(stage) {
          const id = ++requestId;
          writeFileSync(join(home, "request.json"), JSON.stringify({ id, stage }));
          mux(["send-keys", "-t", session, process.platform === "win32" ? "M-v" : "C-v"]);
          await waitFor(`clipboard ${stage} started`, () => existsSync(join(home, `${id}-${stage}.started`)));
          return async () => {
            writeFileSync(join(home, `${id}-${stage}.release`), "release");
            await waitFor(`clipboard ${stage} finished`, () => existsSync(join(home, `${id}-${stage}.finished`)));
            await sleep(400); // Let the resolved promise and scheduled render finish.
          };
        }
        const start = records().length;
        await send("clipboard-smoke seed: say hello briefly.");
        await waitFor("clipboard seed turn", events => turnOutcome(events.slice(start), "completed"));
        const originalSession = records().find(event => event.method === "turn/started")?.payload?.params?.turn?.sessionId;
        if (!originalSession) throw new Error("Missing original session identity");
        for (const stage of ["read", "resize"]) {
          for (const action of ["send", "clone", "resume"]) {
            const release = await begin(stage);
            const offset = records().length;
            if (action === "send") {
              await send(`clipboard-${stage}: say hello briefly.`);
              await waitFor("clipboard intervening turn", events => turnOutcome(events.slice(offset), "completed"));
            } else {
              await send(action === "clone" ? "/clone" : `/resume ${originalSession}`);
              // RPC success precedes transcript replacement. Release only after
              // its final UI status, or stale insertion could be hidden by reset.
              await waitFor(`clipboard ${action}`, events =>
                rpcSucceeded(events.slice(offset), action === "clone" ? "session/fork" : "session/resume") &&
                screen().includes(action === "clone" ? "Cloned to new session" : "Resumed session"));
            }
            await release();
            // A later real keyboard/render event fences the resolved completion;
            // absence alone on an old captured frame would be a false positive.
            const probe = `clipboard-probe-${stage}-${action}`;
            mux(["send-keys", "-t", session, "-l", probe]);
            await waitFor("post-completion editor frame", () => screen().includes(probe));
            if (/\[image #\d+\]/.test(screen())) throw new Error(`Late ${stage} paste leaked after ${action}`);
            mux(["send-keys", "-t", session, "C-u"]);
            await waitFor("probe cleared", () => !screen().includes(probe));
            console.log(`clipboard substitute ${stage}/${action}: no stale marker`);
          }
        }
        for (const stage of ["read", "resize"]) {
          const release = await begin(stage);
          await release();
          await waitFor(`normal ${stage} paste`, () => /\[image #\d+\]/.test(screen()));
          console.log(`=== normal clipboard substitute ${stage} ===\n${screen()}`);
          mux(["send-keys", "-t", session, "C-u"]);
          await waitFor("normal paste cleared", () => !/\[image #\d+\]/.test(screen()));
        }
        break;
      }
      case "fork": {
        const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
        const forkRequests = () => records().filter((event) => event.direction === "tui-to-server" && event.method === "session/fork");
        for (let i = 1; i <= 12; i++) {
          const start = records().length;
          await send(`fork-smoke-${i}: say hello briefly.`);
          await waitFor(`fork seed ${i}`, events => turnOutcome(events.slice(start), "completed"));
        }
        const opened = () => screen().includes("Fork from Message") && screen().includes("Enter fork") && screen().includes("Esc cancel");
        await send("/fork");
        await waitFor("fork wide help and latest point", () => opened() && /›.*fork-smoke-12/.test(screen()));
        console.log("=== fork wide ===\n" + screen());
        mux(["resize-window", "-t", session, "-x", "40", "-y", "24"]);
        await waitFor("fork narrow help and latest point", () => opened() && /›.*fork-smoke-12/.test(screen()));
        console.log("=== fork narrow ===\n" + screen());
        mux(["send-keys", "-t", session, "Up"]);
        await waitFor("previous fork point", () => /›.*fork-smoke-11/.test(screen()));
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("cancel fork", () => !screen().includes("Fork from Message"));
        if (forkRequests().length !== 0) throw new Error("Cancel unexpectedly forked the session");
        await send("/fork");
        await waitFor("fork reopens at latest point", () => opened() && /›.*fork-smoke-12/.test(screen()));
        mux(["send-keys", "-t", session, "Up", "Enter"]);
        await waitFor("confirmed fork", events => rpcSucceeded(events, "session/fork") && screen().includes("Forked to new session"));
        const request = forkRequests();
        const selectedTurn = records().find(event => event.method === "item/completed"
          && event.payload?.params?.item?.item?.type === "userMessage"
          && event.payload.params.item.item.content?.some(block => block.text?.startsWith("fork-smoke-11:")))?.payload.params.item.turnId;
        if (request.length !== 1 || !selectedTurn || request[0].payload.params.atTurnId !== selectedTurn
          || request[0].payload.params.cut !== "through") throw new Error("Fork selected the wrong history point");
        if (screen().includes("Fork from Message")) throw new Error("Fork selector remained open");
        break;
      }
      case "theme": {
        const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
        const titleStyle = () => mux(["capture-pane", "-t", session, "-p", "-e"]).stdout
          .split("\n").find((line) => line.includes("Theme preview"));
        const savedTheme = () => JSON.parse(readFileSync(join(home, "settings.json"), "utf8")).theme;
        const opened = () => {
          const text = screen();
          return text.includes("Theme preview") && text.includes("Enter save") && text.includes("Esc restore");
        };
        await send("/theme");
        await waitFor("theme help and saved selection", () => opened() && /›.*dark/.test(screen()));
        console.log("=== theme wide ===\n" + screen());
        mux(["resize-window", "-t", session, "-x", "40", "-y", "24"]);
        await waitFor("narrow theme help", () => opened() && screen().includes("light"));
        console.log("=== theme narrow ===\n" + screen());
        const darkTitle = titleStyle();
        if (!darkTitle) throw new Error("Missing styled theme title");
        mux(["send-keys", "-t", session, "Down"]);
        await waitFor("theme preview", () => /›.*light/.test(screen()) && titleStyle() !== darkTitle);
        const lightTitle = titleStyle();
        console.log("=== theme preview style ===\n" + lightTitle);
        if (savedTheme() !== "dark") throw new Error("Preview persisted the theme before confirmation");
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("theme restore", () => !screen().includes("Theme preview"));
        if (savedTheme() !== "dark") throw new Error("Cancel changed the saved theme");
        await send("/theme");
        await waitFor("theme reopens at original selection", () => opened() && /›.*dark/.test(screen()) && titleStyle() === darkTitle);
        mux(["send-keys", "-t", session, "Down", "Enter"]);
        await waitFor("theme saved", () => savedTheme() === "light" && screen().includes("Theme: light"));
        if (screen().includes("Theme preview")) throw new Error("Theme selector did not close after save");
        await send("/theme");
        await waitFor("theme reopens at confirmed selection", () => opened() && /›.*light/.test(screen()) && titleStyle() === lightTitle);
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("theme selector dismissed", () => !screen().includes("Theme preview"));
        mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
        break;
      }
      case "effort": {
        const changes = () => records().filter((event) => event.direction === "tui-to-server"
          && event.method === "session/metadata/update"
          && event.payload?.params?.settings?.reasoningEffort).length;
        // Use only the visible pane, not scrollback: closing/reopening assertions
        // must not accidentally match a selector from an earlier frame.
        const screen = () => mux(["capture-pane", "-t", session, "-p"]).stdout;
        const opened = () => {
          const text = screen();
          return text.includes("Reasoning effort") && text.includes("Enter select") && text.includes("Esc cancel");
        };
        const before = changes();
        await send("/effort");
        await waitFor("effort help and current selection", () => opened() && /›.*medium/.test(screen()));
        console.log("=== effort wide ===\n" + screen());
        mux(["resize-window", "-t", session, "-x", "40", "-y", "24"]);
        await waitFor("narrow effort help", () => opened() && /low/.test(screen()) && /high/.test(screen()));
        console.log("=== effort narrow ===\n" + screen());
        mux(["send-keys", "-t", session, "Down"]);
        await waitFor("effort highlight moves", () => /›.*high/.test(screen()));
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("effort cancelled", () => !screen().includes("Reasoning effort"));
        if (changes() !== before) throw new Error("Cancelling effort changed session settings");
        await send("/effort");
        await waitFor("effort reopens at unchanged selection", () => opened() && /›.*medium/.test(screen()));
        mux(["send-keys", "-t", session, "Down", "Enter"]);
        await waitFor("effort selection persists", (events) => {
          const requests = events.filter((event) => event.direction === "tui-to-server"
            && event.method === "session/metadata/update"
            && event.payload?.params?.settings?.reasoningEffort).slice(before);
          return requests.some((request) => request.payload.params.settings.reasoningEffort === "high"
            && events.some((event) => event.direction === "server-to-tui" && event.kind === "response"
              && event.id === request.id && event.payload?.result != null && !event.payload?.error))
            && screen().includes("Thinking level: high");
        });
        if (screen().includes("Reasoning effort")) throw new Error("Effort selector did not close after selection");
        await send("/effort");
        await waitFor("effort reopens at confirmed selection", () => opened() && /›.*high/.test(screen()));
        mux(["send-keys", "-t", session, "Escape"]);
        await waitFor("confirmed selector dismissed", () => !screen().includes("Reasoning effort"));
        mux(["resize-window", "-t", session, "-x", "100", "-y", "30"]);
        break;
      }
      case "activity": {
        // No chat keyboard event precedes these assertions: boot and background
        // reads must paint the dock after the separate startup trust prompt.
        await waitFor("authoritative zero activity without input", (events, text) =>
          rpcSucceeded(events, "task/list") && /Subagents 0/.test(text)
          && /Scheduled 0/.test(text) && /Shells 0/.test(text));
        const initialReads = records().filter((r) => r.method === "task/list").length;
        await waitFor("automatic activity polling without input", (events) =>
          events.filter((r) => r.method === "task/list").length > initialReads);
        break;
      }
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
        await waitFor("heartbeat status", (_events, text) => /No active scheduled task|Scheduled task\s+Status:/i.test(text));
        break;
      case "heartbeat": {
        const marker = `hb-tui-fire-${Date.now()}`;
        await send(`/heartbeat every 1s ${marker}`);
        // A slash-command echo also contains the marker. Require a delivered
        // Native user item rather than matching the echoed command.
        await waitFor("delivered heartbeat", (events) => scheduledDelivered(events, marker));
        await send("/heartbeat clear");
        await waitFor("heartbeat clear", (_events, text) => /Scheduled task cleared/i.test(text));
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
    // The killed product can flush session files for a few beats after
    // kill-session; a cleanup race must never fail a passing run, so retry
    // briefly and then leave the temp dir to the OS.
    for (let attempt = 0; attempt < 10; attempt++) {
      try {
        rmSync(home, { recursive: true, force: true });
        break;
      } catch (error) {
        if (attempt === 9) console.error(`home cleanup left behind (${error.code ?? error.message}): ${home}`);
        await sleep(500);
      }
    }
  }
}

run().catch((error) => { console.error(error.message); process.exitCode = 1; });
