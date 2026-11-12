import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

test("an A-B-A switch cannot publish a snapshot from A's previous attachment", async () => {
  let conn: NativeAgentConnection;
  let version = "old";
  let releaseOldRoster: (() => void) | undefined;
  let holdRoster = false;
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
    if (request.method === "subscription/create") return respond({ subscriptionId: "subscription", cursors: [] });
    if (request.method === "session/items/list") return respond({ data: [{ id: version, createdAt: "2026-11-12T00:00:00Z", seq: 1, revision: 1, state: "completed", item: { type: "userMessage", content: [{ type: "text", text: version }] } }] });
    if (request.method === "agent/list" && holdRoster) {
      holdRoster = false;
      releaseOldRoster = () => respond({ agents: [{ id: "old-child", item: { type: "subAgent", agentSessionId: "old-child", agentNickname: "old" } }] });
      return;
    }
    respond({ data: [], agents: [] });
  } });
  try {
    await conn.switchSession("A", { historyLoadMode: "deferred" });
    holdRoster = true;
    const stale = conn.getInitialSnapshot();
    while (!releaseOldRoster) await new Promise(resolve => setImmediate(resolve));
    await conn.switchSession("B", { historyLoadMode: "deferred" });
    version = "new";
    await conn.switchSession("A", { historyLoadMode: "deferred" });
    const current = await conn.getInitialSnapshot();
    releaseOldRoster();
    const resolved = await stale;
    assert.deepEqual(resolved.messages, current.messages);
    assert.deepEqual(resolved.children, current.children);
  } finally { releaseOldRoster?.(); await conn.dispose(); }
});
