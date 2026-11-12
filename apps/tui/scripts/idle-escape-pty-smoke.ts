/** Real InteractiveMode/Native adapter under a PTY; controlled RPC, no model/server.
 * Pass scenario (idle|active|tree|draft) and a marker path. Once marker.ready exists,
 * send Escape to the terminal (tree: twice, then once to dismiss the selector).
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { NativeAgentConnection } from "../src/native-agent-connection.js";
import { createDevoUiServices, applyDevoAgentDirEnv } from "../src/host.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";
assert.ok(process.stdin.isTTY && process.stdout.isTTY);
const [scenario, marker] = process.argv.slice(2);
assert.ok(["idle", "active", "tree", "draft"].includes(scenario));
const home = fs.mkdtempSync(path.join(os.tmpdir(), "devo-escape-"));
applyDevoAgentDirEnv(home);
initTheme("dark");
let conn: NativeAgentConnection;
let interrupts = 0;
let treeReads = 0;
const push = (method: string, params: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
conn = NativeAgentConnection.create({ cwd: home, writeLine(line) {
  const q = JSON.parse(line);
  if (q.id == null) return;
  const reply = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }) + "\n");
  if (q.method === "session/interrupt") { interrupts++; reply({}); return; }
  if (q.method === "session/tree/read") {
    treeReads++;
    reply({ tree: [{ entry: { id: "u", type: "message", timestamp: new Date().toISOString(), message: { role: "user", content: "Previous question", timestamp: Date.now() } }, children: [] }], leafId: "u" });
    return;
  }
  if (q.method === "agent/list") reply({ agents: [] });
  else reply({ data: [], nextCursor: null });
} });
Object.assign(conn, { sessionId: "one", activeSessionId: "one", sessionName: "Escape fixture" });
const mode = new InteractiveMode({ agentConnection: conn, uiServices: createDevoUiServices({ home, cwd: home }), bindLocalSessionExtensions: false, forceFullscreen: true, agentsViewOwnsStartupNotices: true, excludedBuiltinCommands: ["traces"], version: "smoke" });
try {
  await mode.init();
  if (scenario === "active") push("turn/started", { sessionId: "one", turn: { id: "running" } });
  fs.writeFileSync(marker + ".ready", "ready");
  if (scenario === "draft") {
    await new Promise(resolve => setTimeout(resolve, 250));
    assert.equal((mode as unknown as { editor: { getText(): string } }).editor.getText(), "unsent draft", "first Esc preserves draft");
  }
  await new Promise(resolve => setTimeout(resolve, 1800));
  if (scenario === "draft") assert.equal((mode as unknown as { editor: { getText(): string } }).editor.getText(), "", "second Esc clears draft");
  const rows = (mode as unknown as { chatContainer: { render(width: number): string[] } }).chatContainer.render(100).join("\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  fs.writeFileSync(marker + ".rows", rows);
  assert.equal(interrupts, 1, "tree/modal Esc must not interrupt again");
  assert.equal(treeReads, scenario === "tree" ? 1 : 0);
  assert.equal((rows.match(/Operation aborted/g) ?? []).length, scenario === "active" ? 1 : 0, rows);
  if (scenario === "tree") {
    const view = mode as unknown as { ui: { focusedComponent: unknown }; editor: unknown };
    assert.equal(view.ui.focusedComponent, view.editor, "modal Esc restores editor focus");
  }
} finally {
  await mode.teardownSessionUi();
  await conn.dispose();
  fs.rmSync(home, { recursive: true, force: true });
}
console.log(`PASS ${scenario}`);
