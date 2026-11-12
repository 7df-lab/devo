import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

for (const viaSlash of [false, true]) {
for (const delayedFailure of [false, true]) {
  test(`a switched-away ${viaSlash ? "slash" : "API"} compact ${delayedFailure ? "rejection" : "waiter"} cannot publish into the target`, async () => {
    let conn: NativeAgentConnection;
    let rejectCompact: (() => void) | undefined;
    let admitted = false;
    const events: any[] = [];
    conn = NativeAgentConnection.create({ writeLine(line) {
      const q = JSON.parse(line); if (q.id == null) return;
      const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }) + "\n");
      if (q.method === "session/resume") return respond({ session: { id: q.params.sessionId } });
      if (q.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
      if (q.method === "session/items/list") return respond({ data: Array.from({ length: 4 }, (_, i) => ({ id: `m${i}`, item: { type: "userMessage", content: [{ type: "text", text: `${i}` }] } })) });
      if (q.method === "session/compact/start") {
        assert.equal(q.params.sessionId, "A");
        admitted = true;
        rejectCompact = () => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, error: { code: -32000, message: "source error" } }) + "\n");
        if (!delayedFailure) respond({});
        return;
      }
      respond({ data: [], agents: [] });
    } });
    conn.subscribe(event => events.push(event));
    try {
      await conn.switchSession("A", { historyLoadMode: "deferred" });
      const compaction = viaSlash ? conn.prompt("/compact") : assert.rejects(conn.compact(), /session change/);
      while (!admitted) await new Promise(resolve => setImmediate(resolve));
      await conn.switchSession("B", { historyLoadMode: "deferred" });
      conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "session/statusChanged", params: { sessionId: "B", flags: ["compacting"] } }) + "\n");
      const boundary = events.length;
      if (delayedFailure) rejectCompact!();
      await compaction;
      assert.equal((await conn.getState()).isCompacting, true);
      assert.deepEqual(events.slice(boundary).filter(e => e.type === "session_event" && ["compaction_end", "message_start"].includes(e.event.type)), []);
    } finally { rejectCompact?.(); await conn.dispose(); }
  });
}

}
