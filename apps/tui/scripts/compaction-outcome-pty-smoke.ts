/**
 * Run from apps/tui in a real PTY (tmux/psmux or script):
 * TERM=xterm-256color script -q -e -c 'node --import tsx scripts/compaction-outcome-pty-smoke.ts queued' /tmp/compact-pty.log
 * Scenarios: queued, idle, error, skipped. Optional second argument saves rendered rows.
 * Real Native adapter + InteractiveMode; controlled RPC, no Rust server/provider.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { NativeAgentConnection } from "../src/native-agent-connection.js";
import { createDevoUiServices, applyDevoAgentDirEnv } from "../src/host.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

assert.ok(process.stdin.isTTY && process.stdout.isTTY, "Run inside a real PTY");
const scenario = process.argv[2] ?? "queued";
assert.ok(["queued", "idle", "error", "skipped"].includes(scenario), `Unknown scenario: ${scenario}`);
const outputPath = process.argv[3];
const home = fs.mkdtempSync(path.join(os.tmpdir(), "devo-compact-pty-"));
applyDevoAgentDirEnv(home);
initTheme("dark");
const services = createDevoUiServices({ home, cwd: home });
const user = (id: string, text: string, seq: number) => ({
  id, seq, revision: 1, state: "completed", sessionId: "one", turnId: "previous",
  createdAt: "2026-11-12T10:00:00Z",
  item: { type: "userMessage", content: [{ type: "text", text }] },
});
const history: unknown[] = Array.from({ length: 4 }, (_, i) => user(`old${i}`, `Previous message ${i}`, i + 1));
const compactItem = {
  id: "compact", seq: 5, revision: 1, state: "completed", sessionId: "one", turnId: "compact-turn",
  createdAt: "2026-11-12T10:01:00Z",
  item: {
    type: "contextCompaction", trigger: "manual", summary: "Retained discussion",
    before: { totalTokens: 12000, measured: true }, after: { totalTokens: 3000, measured: true },
  },
};
let conn: NativeAgentConnection;
let completed = false;
const timers: ReturnType<typeof setTimeout>[] = [];
const later = (callback: () => void, delay: number) => timers.push(setTimeout(callback, delay));
const push = (method: string, params: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
conn = NativeAgentConnection.create({
  cwd: home,
  writeLine(line) {
    const q = JSON.parse(line);
    if (q.id == null) return;
    const reply = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }) + "\n");
    if (q.method === "session/items/list") {
      later(() => reply({ data: [...history], nextCursor: null }), 30);
      return;
    }
    if (q.method === "session/compact/start") {
      if (scenario === "error") {
        conn.pushChunk(JSON.stringify({
          jsonrpc: "2.0", id: q.id,
          error: { code: -32000, message: "cannot compact while a turn is active or queued" },
        }) + "\n");
        return;
      }
      reply({});
      later(() => {
        const outcomeItem = scenario === "skipped" ? {
          ...compactItem, state: "completed",
          item: { ...compactItem.item, summary: "Compaction failed: Session is too short to compact" },
        } : compactItem;
        history.push(outcomeItem);
        completed = true;
        push("item/completed", { item: outcomeItem });
        if (scenario === "skipped") {
          // Native reports the completed attempt after its skipped/failed item.
          // This envelope lacks trigger; it must not overwrite the warning.
          push("context/compactionCompleted", { sessionId: "one", itemId: "compact" });
        }
        push("turn/completed", { sessionId: "one", turn: { id: "compact-turn", kind: "compaction", status: "completed" } });
        if (scenario === "queued") {
          const next = user("queued", "这确实是一个方向", 6);
          next.turnId = "next-turn";
          history.push(next);
          push("turn/started", { sessionId: "one", turn: { id: "next-turn", kind: "regular", status: "running" } });
          push("item/started", { item: next });
          push("item/completed", { item: next });
        }
      }, 100);
      return;
    }
    if (q.method === "session/turns/list") {
      reply({
        data: completed ? [
          { id: "compact-turn", kind: "compaction", status: "completed" },
          ...(scenario === "queued" ? [{ id: "next-turn", kind: "regular", status: "running" }] : []),
        ] : [], nextCursor: null,
      });
      return;
    }
    if (q.method === "agent/list") reply({ agents: [] });
    else reply({ data: [], nextCursor: null });
  },
});
// Simulated selected session; requests above never create a server session/model.
Object.assign(conn, { sessionId: "one", activeSessionId: "one", sessionName: "Compaction outcome fixture" });
const mode = new InteractiveMode({
  agentConnection: conn, uiServices: services, bindLocalSessionExtensions: false, forceFullscreen: true,
  agentsViewOwnsStartupNotices: true, excludedBuiltinCommands: ["traces"], version: "smoke",
  appTitle: "Devo compaction outcome verification",
});
let failure: unknown;
try {
  await mode.init();
  await conn.prompt("/compact");
  await new Promise(resolve => setTimeout(resolve, 500));
  const chat = (mode as unknown as { chatContainer: { render(width: number): string[] } }).chatContainer;
  const rows = chat.render(100).join("\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  if (outputPath) fs.writeFileSync(outputPath, rows);
  if (scenario === "skipped") {
    assert.equal((rows.match(/Session is too short to compact/g) ?? []).length, 1, rows);
    assert.doesNotMatch(rows, /Compaction · completed|Compaction finished/);
  } else if (scenario === "error") {
    assert.equal((rows.match(/cannot compact while a turn is active or queued/g) ?? []).length, 1, rows);
    assert.doesNotMatch(rows, /Compaction · completed|Compaction finished/);
  } else {
    assert.equal((rows.match(/Compaction · completed/g) ?? []).length, 1, rows);
    assert.doesNotMatch(rows, /Compaction finished\./);
    if (scenario === "queued") {
      assert.equal((rows.match(/这确实是一个方向/g) ?? []).length, 1, rows);
      assert.ok(rows.indexOf("Compaction · completed") < rows.indexOf("这确实是一个方向"), rows);
    }
  }
} catch (error) {
  failure = error;
} finally {
  for (const timer of timers) clearTimeout(timer);
  await mode.teardownSessionUi();
  await conn.dispose();
  fs.rmSync(home, { recursive: true, force: true });
}
if (failure) throw failure;
console.log(`PASS ${scenario}: real PTY + real Native adapter + InteractiveMode, controlled RPC`);
