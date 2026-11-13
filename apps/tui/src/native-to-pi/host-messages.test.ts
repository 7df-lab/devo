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

for (const status of ["waiting_for_input", "completed", "failed", "interrupted", "canceled", "closed"]) {
  test(`child ${status}: hide generated preamble in live and history, preserve result and status`, () => {
    const origin = { source: "agent", sender: "ses_child", senderName: "Reviewer", eventId: `child:ses_child:turn_1:${status}`, delivery: "followUp" };
    const text = `[system] Child agent /root/reviewer (ses_child) ${status}.\nUseful result\n[system] quoted content stays`;
    const envelope = { createdAt: "2026-11-09T00:00:00Z", item: { type: "userMessage", origin, content: [{ type: "text", text }] } };
    const expected = { role: "custom", customType: "Agent message", display: true, details: { origin }, timestamp: Date.parse(envelope.createdAt), content: [{ type: "text", text: `From: Reviewer · ${status}` }, { type: "text", text: "Useful result\n[system] quoted content stays" }] };
    assert.deepEqual(nativeItemToAgentMessages(envelope), [expected]);
    assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), [{ type: "message_start", message: expected }, { type: "message_end", message: expected }]);
    assert.equal(envelope.item.content[0].text, text, "projection must not mutate durable/model text");
  });
}

test("system-looking user and explicit agent messages are never stripped", () => {
  const text = "[system] Child agent /root/reviewer (ses_child) completed.\nActual quoted message";
  for (const origin of [undefined, { source: "agent", sender: "ses_child", eventId: "explicit-message" }, { source: "agent", sender: "other", eventId: "child:ses_child:turn_1:completed" }, { source: "backgroundTask", sender: "ses_child", eventId: "child:ses_child:turn_1:completed" }, { source: "agent", sender: "ses_child", eventId: "child:ses_child:turn_1:failed" }]) {
    const result = nativeItemToAgentMessages({ item: { type: "userMessage", content: [{ type: "text", text }], ...(origin ? { origin } : {}) } });
    assert.ok(JSON.stringify(result).includes(JSON.stringify(text).slice(1, -1)));
  }
});

test("empty completion retains useful status and subsequent text parts", () => {
  const origin = { source: "agent", sender: "ses_child", eventId: "child:ses_child:turn_1:failed" };
  const [result] = nativeItemToAgentMessages({ item: { type: "userMessage", origin, content: [{ type: "text", text: "[system] Child agent /root/reviewer (ses_child) failed.\n" }, { type: "text", text: "Additional diagnostic" }] } });
  assert.deepEqual(result.content, [{ type: "text", text: "From: ses_child · failed" }, { type: "text", text: "" }, { type: "text", text: "Additional diagnostic" }]);
});
