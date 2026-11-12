import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

for (const order of ["old-first", "new-first"] as const) {
  test(`latest fork intent wins when responses arrive ${order}`, async () => {
    let conn: NativeAgentConnection;
    const releases: Array<() => void> = [];
    conn = NativeAgentConnection.create({ writeLine(line) {
      const request = JSON.parse(line);
      if (request.id == null) return;
      const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
      if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
      if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
      if (request.method === "session/fork") {
        assert.equal(request.params.sessionId, "A");
        releases.push(() => respond({ session: { id: request.params.atTurnId } }));
        return;
      }
      respond({ data: [], agents: [] });
    } });
    try {
      await conn.switchSession("A", { historyLoadMode: "deferred" });
      const older = conn.fork("turn_older");
      while (releases.length < 1) await new Promise(resolve => setImmediate(resolve));
      const newer = conn.fork("turn_newer");
      while (releases.length < 2) await new Promise(resolve => setImmediate(resolve));
      if (order === "old-first") {
        releases[0]();
        assert.deepEqual(await older, { cancelled: true });
        releases[1]();
        assert.deepEqual(await newer, { cancelled: false });
      } else {
        releases[1]();
        assert.deepEqual(await newer, { cancelled: false });
        releases[0]();
        assert.deepEqual(await older, { cancelled: true });
      }
      assert.equal((await conn.getState()).sessionId, "turn_newer");
    } finally { for (const release of releases) release(); await conn.dispose(); }
  });
}

test("a rejected fork retains current history and permits a new initial snapshot", async () => {
  let conn: NativeAgentConnection;
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
    if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
    if (request.method === "session/items/list") return respond({ data: [{ id: "item_A", createdAt: "2026-11-12T00:00:00Z", seq: 1, revision: 1, state: "completed", item: { type: "userMessage", content: [{ type: "text", text: "keep this history" }] } }] });
    if (request.method === "session/fork") {
      conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32603, message: "fork failed" } }) + "\n");
      return;
    }
    respond({ data: [], agents: [] });
  } });
  try {
    await conn.switchSession("A", { historyLoadMode: "deferred" });
    const before = await conn.getInitialSnapshot();
    assert.ok(before.messages.length > 0);
    await assert.rejects(conn.fork("turn_A"), /fork failed/);
    const after = await conn.getInitialSnapshot();
    assert.deepEqual(after.messages, before.messages);
    assert.equal(after.state.sessionId, "A");
  } finally { await conn.dispose(); }
});
