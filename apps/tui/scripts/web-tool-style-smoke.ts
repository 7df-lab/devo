/** Run inside tmux: node --import tsx scripts/web-tool-style-smoke.ts.
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
const home = fs.mkdtempSync(path.join(os.tmpdir(), "devo-web-style-smoke-"));
applyDevoAgentDirEnv(home);
const services = createDevoUiServices({ home, cwd: home });
initTheme("dark");
const item = (id: string, seq: number, text: string, type = "userMessage", state = "completed", revision = 1) => ({
  id, seq, revision, state, sessionId: "child", turnId: "turn", createdAt: "2026-11-11T08:00:00Z",
  item: type === "assistantMessage" ? { type, text } : { type, content: [{ type: "text", text }] },
});
const old = item("old", 1, "Compare Python and web tool styling");
const tools = [
  { id: "python", seq: 2, item: { type: "toolCall", callId: "python", toolName: "ipython", input: { code: 'print("Hello")' } } },
  { id: "python-result", seq: 3, item: { type: "toolResult", callId: "python", output: "Hello" } },
  { id: "search", seq: 4, item: { type: "hostedToolCall", callId: "search", toolName: "web_search", input: { query: "Rust async" }, output: [{ title: "Rust guide", url: "https://example.test/rust", snippet: "Async guide excerpt" }] } },
  { id: "fetch", seq: 5, item: { type: "hostedToolCall", callId: "fetch", toolName: "web_fetch", input: { url: "https://example.test/rust" }, output: "Fetched page text" } },
].map(tool => ({ ...tool, state: "completed", sessionId: "child", turnId: "turn", createdAt: "2026-11-12T08:00:00Z" }));
let connection: NativeAgentConnection;
connection = NativeAgentConnection.create({ cwd: home, writeLine(line) {
  const request = JSON.parse(line);
  if (request.id == null) return;
  const reply = (result: unknown) => connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n`);
  if (request.method === "session/items/list") {
    reply({ data: [old, ...tools], nextCursor: null });
    return;
  }
  if (request.method === "session/turns/list" || request.method === "model/list") reply({ data: [], nextCursor: null });
  else if (request.method === "agent/list") reply({ agents: [] });
  else reply({});
} });
// The fixture represents an already selected child. No actual session or model is created.
Object.assign(connection, { sessionId: "child", activeSessionId: "child", sessionName: "Web style fixture" });
const mode = new InteractiveMode({ agentConnection: connection, uiServices: services,
  bindLocalSessionExtensions: false, forceFullscreen: true, agentsViewOwnsStartupNotices: true,
  excludedBuiltinCommands: ["traces"], version: "smoke", appTitle: "Devo web style verification" });
let failure: unknown;
try {
  await mode.init();
  await new Promise(resolve => setTimeout(resolve, 20));
  await new Promise(resolve => setTimeout(resolve, 300));
  const chat = (mode as unknown as { chatContainer: { render(width: number): string[] } }).chatContainer;
  const rows = chat.render(100).join("\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  assert.match(rows, /✓ web_search/);
  assert.match(rows, /✓ web_fetch/);
  assert.match(rows, /python/);
  // Leave the real terminal interactive for Ctrl+O expansion and resize checks.
  await new Promise(resolve => setTimeout(resolve, 20000));
} catch (error) { failure = error; }
finally {
  await mode.teardownSessionUi();
  await connection.dispose();
  fs.rmSync(home, { recursive: true, force: true });
}
if (failure) throw failure;
console.log(`PASS: controlled Native web/Python transcript in real TTY`);
