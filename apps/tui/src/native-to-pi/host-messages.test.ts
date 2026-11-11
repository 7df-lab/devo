import { test } from "node:test";
import assert from "node:assert/strict";
import { nativeItemToAgentMessages } from "./messages.js";
import { createStreamState, nativeNotificationToPiEvents } from "./events.js";

for (const [source, label] of Object.entries({ agent: "Agent message", backgroundTask: "Background notification", schedule: "Scheduled notification" })) {
  for (const delivery of ["followUp", "steer"]) {
    test(`${source}/${delivery} is labeled non-human in live and replay`, () => {
      const origin = { source, delivery, eventId: "event-1", sender: "worker-1" };
      const envelope = { id: "item-1", createdAt: "2026-11-09T00:00:00Z", item: {
        type: "userMessage", content: [{ type: "text", text: "Result ready" }], origin,
      } };
      const expected = {
        role: "custom", customType: label, display: true, details: { origin },
        content: [{ type: "text", text: "From: worker-1" }, { type: "text", text: "Result ready" }],
        timestamp: Date.parse(envelope.createdAt),
      };
      assert.deepEqual(nativeItemToAgentMessages(envelope), [expected]);
      assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), [
        { type: "message_start", message: expected }, { type: "message_end", message: expected },
      ]);
    });
  }
}

test("missing origin preserves human messages including legacy text prefixes", () => {
  const envelope = { createdAt: "2026-11-09T00:00:00Z", item: { type: "userMessage", content: [{ type: "text", text: "[Agent message] still human" }] } };
  const expected = { role: "user", content: envelope.item.content, timestamp: Date.parse(envelope.createdAt) };
  assert.deepEqual(nativeItemToAgentMessages(envelope), [expected]);
  assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), [
    { type: "message_start", message: expected }, { type: "message_end", message: expected },
  ]);
});

for (const [source, label] of Object.entries({ agent: "Agent message", backgroundTask: "Background notification", schedule: "Scheduled notification" })) {
  test(`${source} pending notification keeps a sender label`, () => {
    const origin = { source, sender: "worker-1", eventId: "event-1", delivery: "followUp" };
    assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [{ preview: "Result ready", origin }] }, createStreamState()), [{
      type: "session_action_update", actions: { queuedCount: 1, steering: [], followUps: [`[${label} · worker-1] Result ready`], ordered: [{ id: "", text: `[${label} · worker-1] Result ready`, delivery: "followUp", blocked: false, editable: false }] },
    }]);
  });
}
