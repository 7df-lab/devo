import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

function harness() {
  let conn: NativeAgentConnection;
  let pendingQueue: { respond: (result: unknown) => void; reject: () => void } | undefined;
  let holdQueue = false;
  const entry = (sessionId: string) => ({ queueItemId: `${sessionId}-queued`, position: 0, preview: `${sessionId} queued`, delivery: "followUp" });
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line); if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
    if (request.method === "subscription/create") return respond({ subscriptionId: "subscription", cursors: [] });
    if (request.method === "session/queue/list") {
      if (holdQueue) {
        holdQueue = false;
        pendingQueue = { respond, reject: () => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: "offline" } }) + "\n") };
      } else respond({ entries: [entry(request.params.sessionId)], revision: 1 });
      return;
    }
    respond({ data: [], agents: [] });
  } });
  return { conn, entry, hold: () => { holdQueue = true; }, pending: () => pendingQueue };
}

for (const failed of [false, true]) {
  test(`switch clears source compaction and queue while target queue ${failed ? "fails" : "loads"}`, async () => {
    const h = harness();
    try {
      await h.conn.switchSession("A", { historyLoadMode: "deferred" });
      h.conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "session/statusChanged", params: { sessionId: "A", flags: ["compacting"] } }) + "\n");
      const source = await h.conn.getState();
      assert.equal(source.isCompacting, true);
      assert.deepEqual(source.sessionActions?.followUps, ["A queued"]);
      h.hold();
      await h.conn.switchSession("B", { historyLoadMode: "deferred" });
      const target = await h.conn.getState();
      assert.equal(target.isCompacting, false);
      assert.deepEqual(target.sessionActions?.followUps, []);
      assert.equal(target.sessionActions?.queuedCount, 0);
      if (failed) h.pending()!.reject();
      else h.pending()!.respond({ entries: [h.entry("B")], revision: 1 });
      await new Promise(resolve => setImmediate(resolve));
      assert.deepEqual((await h.conn.getState()).sessionActions?.followUps, failed ? [] : ["B queued"]);
    } finally { h.pending()?.reject(); await h.conn.dispose(); }
  });
}

test("old A queue read cannot restore previews after returning to A when the current read fails", async () => {
  const h = harness();
  try {
    await h.conn.switchSession("A", { historyLoadMode: "deferred" });
    h.hold();
    const oldRead = (h.conn as any).refreshQueueFromServer();
    const oldResponse = h.pending()!;
    await h.conn.switchSession("B", { historyLoadMode: "deferred" });
    h.hold();
    await h.conn.switchSession("A", { historyLoadMode: "deferred" });
    h.pending()!.reject();
    await new Promise(resolve => setImmediate(resolve));
    oldResponse.respond({ entries: [h.entry("A")], revision: 1 });
    assert.equal(await oldRead, false);
    assert.deepEqual((await h.conn.getState()).sessionActions?.followUps, []);
  } finally { h.pending()?.reject(); await h.conn.dispose(); }
});

test("target compaction notifications still update the freshly scoped state", async () => {
  const h = harness();
  try {
    await h.conn.switchSession("A", { historyLoadMode: "deferred" });
    await h.conn.switchSession("B", { historyLoadMode: "deferred" });
    h.conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "session/statusChanged", params: { sessionId: "B", flags: ["compacting"] } }) + "\n");
    assert.equal((await h.conn.getState()).isCompacting, true);
    h.conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "session/statusChanged", params: { sessionId: "A", flags: [] } }) + "\n");
    assert.equal((await h.conn.getState()).isCompacting, true);
  } finally { await h.conn.dispose(); }
});
