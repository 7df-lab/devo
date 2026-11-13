import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { nativeItemsToAgentMessages } from "./native-to-pi/messages.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";

for (const item of [
  { type: "refinement", refinementId: "r1", summary: "Consolidated guidance", changes: ["Update Memory:m1"] },
  { type: "refinement", refinementId: "r2", summary: "Harness is already clean", changes: [] },
  { type: "warning", code: "refinement_not_applied", message: "Refinement r3 was not applied: provider failed" },
]) {
  test(`refinement outcome stays visible live and on restore: ${item.type}/${item.refinementId}`, () => {
    const envelope = { id: "result", createdAt: "2026-01-01T00:00:00Z", item };
    const state = createStreamState();
    state.imAssistantStreamOpen = true;
    state.openAssistantItem = "ongoing";
    const events = nativeNotificationToPiEvents("item/completed", { item: envelope }, state);
    assert.equal(events.length, 1);
    assert.equal(events[0].type, "message_start");
    const restored = nativeItemsToAgentMessages([envelope]);
    assert.deepEqual(events[0].message, restored[0]);
    assert.equal(restored[0].customType, "Refinement");
    assert.match(String(restored[0].content), item.type === "warning" ? /provider failed/ : item.changes?.length ? /1 changes/ : /no changes needed/);
    assert.equal(state.imAssistantStreamOpen, true);
    assert.equal(state.openAssistantItem, "ongoing");
  });
}

test("refine admission does not emit a false completion", async () => {
  const events: any[] = [];
  const conn = NativeAgentConnection.create({ writeLine(line) {
    const q = JSON.parse(line);
    if (q.id == null) return;
    const result = q.method === "session/resume" ? { session: { id: "A" } }
      : q.method === "session/refine/run" ? { scheduled: true, refinementId: "r1", note: "Refinement queued" }
      : { data: [], agents: [], subscriptionId: "sub", cursors: [] };
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: q.id, result }) + "\n");
  } });
  conn.subscribe(event => { if (event.type === "session_event") events.push(event.event); });
  try {
    await conn.switchSession("A", { historyLoadMode: "deferred" });
    await conn.refine();
    assert.equal(events.filter(e => e.type === "refine_complete").length, 0);
    assert.ok(events.some(e => e.message?.content === "Refinement queued"));
  } finally { await conn.dispose(); }
});
