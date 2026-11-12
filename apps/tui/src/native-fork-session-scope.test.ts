import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

for (const next of ["B", "A-B-A", "dispose", "unchanged"] as const) {
  test(`fork response respects the latest attachment: ${next}`, async () => {
    let conn: NativeAgentConnection;
    let release: (() => void) | undefined;
    conn = NativeAgentConnection.create({ writeLine(line) {
      const request = JSON.parse(line);
      if (request.id == null) return;
      const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
      if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
      if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
      if (request.method === "session/fork") {
        assert.equal(request.params.sessionId, "A");
        release = () => respond({ session: { id: "A-fork" } });
        return;
      }
      respond({ data: [], agents: [] });
    } });
    try {
      await conn.switchSession("A", { historyLoadMode: "deferred" });
      const pending = conn.fork("turn_A");
      while (!release) await new Promise(resolve => setImmediate(resolve));
      if (next === "dispose") await conn.dispose();
      else if (next !== "unchanged") {
        await conn.switchSession("B", { historyLoadMode: "deferred" });
        if (next === "A-B-A") await conn.switchSession("A", { historyLoadMode: "deferred" });
      }
      release();
      assert.deepEqual(await pending, { cancelled: next !== "unchanged" });
      assert.equal((await conn.getState()).sessionId, next === "unchanged" ? "A-fork" : next === "B" ? "B" : "A");
    } finally { release?.(); await conn.dispose(); }
  });
}

test("a session switch during item-to-turn lookup never forks the new session", async () => {
  let conn: NativeAgentConnection;
  let release: (() => void) | undefined;
  let forkRequests = 0;
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/resume") return respond({ session: { id: request.params.sessionId } });
    if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
    if (request.method === "session/tree/read") { release = () => respond({ nodes: [], leafId: "item_A" }); return; }
    if (request.method === "session/fork") { forkRequests += 1; return respond({ session: { id: "wrong-fork" } }); }
    respond({ data: [], agents: [] });
  } });
  try {
    await conn.switchSession("A", { historyLoadMode: "deferred" });
    const pending = conn.fork("item_A");
    while (!release) await new Promise(resolve => setImmediate(resolve));
    await conn.switchSession("B", { historyLoadMode: "deferred" });
    release();
    assert.deepEqual(await pending, { cancelled: true });
    assert.equal(forkRequests, 0);
    assert.equal((await conn.getState()).sessionId, "B");
  } finally { release?.(); await conn.dispose(); }
});
