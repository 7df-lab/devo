import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

for (const queued of [false, true]) {
  for (const outcome of ["success", "failure", "skipped"] as const) {
    test(`manual compact has a single ${outcome} outcome, queued=${queued}`, async () => {
      let conn: NativeAgentConnection;
      const events: any[] = [];
      let historyReads = 0;
      let retrySuccess = false;
      const notify = (method: string, params: any) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method, params: { sessionId: "A", ...params } }) + "\n");
      conn = NativeAgentConnection.create({ writeLine(line) {
        const q = JSON.parse(line);
        if (q.id == null) return;
        const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }) + "\n");
        if (q.method === "session/resume") return respond({ session: { id: "A" } });
        if (q.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
        if (q.method === "session/items/list") {
          historyReads++;
          return respond({ data: Array.from({ length: 4 }, (_, i) => ({ id: `m${i}`, item: { type: "userMessage", content: [{ type: "text", text: `${i}` }] } })) });
        }
        if (q.method === "session/compact/start") {
          respond({});
          if (outcome === "skipped" && !retrySuccess) {
            notify("item/completed", { item: { id: "compact", state: "completed", item: {
              type: "contextCompaction", trigger: "manual", summary: "Compaction failed: Session is too short to compact",
            } } });
            // Real server skipped sequence ends with a success-shaped ack.
            notify("context/compactionCompleted", { itemId: "compact" });
          } else notify(outcome === "success" || retrySuccess ? "context/compactionCompleted" : "context/compactionFailed", {
            trigger: "manual", itemId: "compact", message: "provider disconnected",
          });
          // The server may drain the FIFO before the compact() waiter resumes.
          if (queued) notify("item/started", { item: { id: "followup", item: { type: "userMessage", content: [{ type: "text", text: "这确实是一个方向" }] } } });
          return;
        }
        respond({ data: [], agents: [] });
      } });
      conn.subscribe(event => { if (event.type === "session_event") events.push(event.event); });
      try {
        await conn.switchSession("A", { historyLoadMode: "deferred" });
        historyReads = 0;
        await conn.prompt("/compact");
        const ends = events.filter(e => e.type === "compaction_end");
        assert.equal(ends.length, 1);
        assert.equal(Boolean(ends[0].result), outcome === "success");
        assert.equal(ends[0].errorSeverity, outcome === "success" ? undefined : outcome === "skipped" ? "warning" : "error");
        assert.deepEqual(events.filter(e => e.type === "message_start" && e.message.customType === "session_slash_command_result"), []);
        const followup = events.findIndex(e => e.type === "message_start" && e.message.role === "user");
        if (queued) {
          assert.ok(followup > events.indexOf(ends[0]));
          assert.equal(events.filter(e => e.type === "message_start" && e.message.role === "user").length, 1);
        } else assert.equal(followup, -1);
        // Transcript hydration belongs to InteractiveMode's compaction_end.
        assert.equal(historyReads, 1);
        if (outcome === "skipped") {
          events.length = 0;
          retrySuccess = true;
          await conn.prompt("/compact");
          const retriedEnds = events.filter(e => e.type === "compaction_end");
          assert.equal(retriedEnds.length, 1);
          assert.ok(retriedEnds[0].result, "A new compaction must not inherit the skipped item fence");
        }
      } finally { await conn.dispose(); }
    });
  }
}
