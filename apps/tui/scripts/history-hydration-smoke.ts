/** Run inside a real PTY: node --import tsx scripts/history-hydration-smoke.ts [--stream].
 * Controlled JSON-RPC transport, real Native adapter + InteractiveMode. No provider/network.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { NativeAgentConnection } from "../src/native-agent-connection.js";
import { createDevoUiServices, applyDevoAgentDirEnv } from "../src/host.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

assert.ok(process.stdin.isTTY && process.stdout.isTTY, "Run this smoke check inside tmux/psmux or a real PTY");
const stream = process.argv.includes("--stream");
const home = fs.mkdtempSync(path.join(os.tmpdir(), "devo-hydration-smoke-"));
applyDevoAgentDirEnv(home);
const services = createDevoUiServices({ home, cwd: home });
initTheme("dark");
const item = (id: string, seq: number, text: string, type = "userMessage", state = "completed", revision = 1) => ({
  id, seq, revision, state, sessionId: "child", turnId: "turn", createdAt: "2026-11-11T08:00:00Z",
  item: type === "assistantMessage" ? { type, text } : { type, content: [{ type: "text", text }] },
});
const old = item("old", 1, "HISTORY_FIRST");
const live = stream ? item("live", 2, "PREFIX_", "assistantMessage", "running") : item("live", 2, "LIVE_SECOND");
let connection: NativeAgentConnection;
let injected = false;
const timers: ReturnType<typeof setTimeout>[] = [];
const later = (callback: () => void, delay: number) => { timers.push(setTimeout(callback, delay)); };
const push = (method: string, params: unknown) => connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", method, params })}\n`);
connection = NativeAgentConnection.create({ cwd: home, writeLine(line) {
  const request = JSON.parse(line);
  if (request.id == null) return;
  const reply = (result: unknown) => connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`);
  if (request.method === "session/items/list") {
    if (!injected) {
      injected = true;
      if (stream) {
        later(() => push("item/assistantMessage/delta", { sessionId: "child", itemId: "live", delta: "LIVE_SECOND" }), 15);
        later(() => push("item/completed", { item: item("live", 2, "PREFIX_LIVE_SECOND", "assistantMessage", "completed", 2) }), 250);
      } else {
        later(() => push("item/started", { item: live }), 15);
        later(() => push("item/completed", { item: live }), 20);
      }
    }
    later(() => reply({ data: [old, live], nextCursor: null }), 120);
    return;
  }
  if (request.method === "session/turns/list" || request.method === "model/list") reply({ data: [], nextCursor: null });
  else if (request.method === "agent/list") reply({ agents: [] });
  else reply({});
} });
// The fixture represents an already selected child. No actual session or model is created.
Object.assign(connection, { sessionId: "child", activeSessionId: "child", sessionName: "Hydration fixture" });
const mode = new InteractiveMode({ agentConnection: connection, uiServices: services,
  bindLocalSessionExtensions: false, forceFullscreen: true, agentsViewOwnsStartupNotices: true,
  excludedBuiltinCommands: ["traces"], version: "smoke", appTitle: "Devo hydration verification" });
let failure: unknown;
try {
  await mode.init();
  await new Promise(resolve => setTimeout(resolve, 20));
  if (stream) {
    const intermediate = (mode as unknown as { chatContainer: { render(width: number): string[] } }).chatContainer.render(100).join("\n");
    assert.ok(intermediate.includes("PREFIX_LIVE_SECOND"), "Streaming prefix must survive before completion");
  }
  await new Promise(resolve => setTimeout(resolve, 300));
  const rows = (mode as unknown as { chatContainer: { render(width: number): string[] } }).chatContainer
    .render(100).join("\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  assert.ok(rows.indexOf("HISTORY_FIRST") >= 0);
  assert.ok(rows.indexOf(stream ? "PREFIX_LIVE_SECOND" : "LIVE_SECOND") > rows.indexOf("HISTORY_FIRST"), rows);
  assert.equal((rows.match(/LIVE_SECOND/g) ?? []).length, 1, rows);
} catch (error) { failure = error; }
finally {
  for (const timer of timers) clearTimeout(timer);
  await mode.teardownSessionUi();
  await connection.dispose();
}
if (failure) throw failure;
console.log(`PASS: controlled Native ${stream ? "streaming" : "completed"} history/live hydration in real PTY`);
