import { test } from "node:test";
import assert from "node:assert/strict";
import { stripVTControlCharacters } from "node:util";
import { CustomMessageComponent } from "../../lib/coding-agent/src/modes/interactive/components/custom-message.js";
import type { CustomMessage } from "../../lib/coding-agent/src/core/messages.js";
import { initTheme } from "../../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { hostMessageForkText, queueEditableText, queuePreviewText, queueSelectionText } from "../native-connection-ops.js";
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
        content: [
          ...(source === "agent" ? [{ type: "text", text: "From: worker-1" }] : []),
          { type: "text", text: "Result ready" },
        ],
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

for (const [source, label] of Object.entries({ agent: "worker-1", backgroundTask: "Background", schedule: "Scheduled" })) {
  test(`${source} pending notification has a compact label`, () => {
    const origin = { source, sender: "worker-1", eventId: "event-1", delivery: "followUp" };
    const text = `${label}: Result ready`;
    assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [{ preview: "Result ready", origin }] }, createStreamState()), [{
      type: "session_action_update", actions: { queuedCount: 1, steering: [], followUps: [text], ordered: [{ id: "", text, delivery: "followUp", blocked: false, editable: false }] },
    }]);
  });
}

const owner = "ses_0123456789abcdef0123456789abcdef";
const scheduleOrigin = { source: "schedule", sender: owner, eventId: "schedule:job:123", delivery: "steer" };
const shellOrigin = { source: "backgroundTask", sender: `${owner}:bash:handle-1`, eventId: `background:${owner}:bash:handle-1:completed`, delivery: "steer" };

function hostItem(origin: Record<string, unknown>, text: string) {
  return { createdAt: "2026-11-09T00:00:00Z", item: { type: "userMessage", origin, content: [{ type: "text", text }] } };
}

function hostQueueEntry(origin: Record<string, unknown>, text: string, preview = text.slice(0, 80)) {
  return { origin, queueItemId: "opaque-queue-id", position: 1, delivery: "steer" as const, preview, input: [{ type: "text", text }] };
}

test("scheduled notification uses the full instruction without the opaque sender", () => {
  const instruction = "Check the current build and summarize the errors.\nSkip network checks.";
  const entry = hostQueueEntry(scheduleOrigin, instruction, "Check the current…");
  const preview = "Scheduled: Check the current build and summarize the errors.";
  const text = `${preview}\nSkip network checks.`;
  assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [entry] }, createStreamState()), [{
    type: "session_action_update", actions: {
      queuedCount: 1, steering: [text], followUps: [], ordered: [{
        id: entry.queueItemId, text, preview, delivery: "steer", blocked: false, editable: false,
      }],
    },
  }]);
  assert.equal(hostMessageForkText(scheduleOrigin, instruction), `[Scheduled notification] ${instruction}`);
  const [message] = nativeItemToAgentMessages(hostItem(scheduleOrigin, instruction));
  assert.deepEqual(message.content, [{ type: "text", text: instruction }]);
  assert.deepEqual(message.details, { origin: scheduleOrigin });
  const named = { ...scheduleOrigin, senderName: "Build check" };
  assert.equal(queuePreviewText({ ...entry, origin: named }), "Scheduled · Build check: Check the current build and summarize the errors.");
});

test("shell result has a compact preview and literal output in live and replay", () => {
  initTheme("dark");
  const raw = JSON.stringify({ status: "ok", result: {
    handle_id: "handle-1", running: false, command: "npm test", exit_code: 0,
    output: "\u001b[2J**literal**\r\n  second line\n", error: null,
  } });
  const envelope = hostItem(shellOrigin, raw);
  const entry = hostQueueEntry(shellOrigin, raw);
  const summary = "exit 0 · npm test";
  const output = "**literal**\n  second line";
  assert.equal(queuePreviewText(entry), `Shell: ${summary}`);
  assert.equal(queueSelectionText(entry), `Shell: ${summary}\n${output}`);
  assert.equal(queueEditableText(entry), undefined);
  assert.equal(queuePreviewText({ ...entry, input: undefined }), "Background");
  assert.equal(hostMessageForkText(shellOrigin, raw), `Shell: ${summary}`);

  const message = {
    role: "custom", customType: "Background shell", display: true, details: { origin: shellOrigin },
    content: `${summary}\n${output}`, timestamp: Date.parse(envelope.createdAt),
  };
  assert.deepEqual(nativeItemToAgentMessages(envelope), [message]);
  assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), [
    { type: "message_start", message }, { type: "message_end", message },
  ]);
  assert.equal(envelope.item.content[0].text, raw);
  assert.equal(entry.input[0].text, raw);
  const rendered = new CustomMessageComponent(message as CustomMessage).render(60)
    .map(line => stripVTControlCharacters(line).trimEnd()).filter(Boolean);
  assert.deepEqual(rendered, [" [Background shell]", " exit 0 · npm test", " **literal**", "   second line"]);

  // A human or another host source can quote the identical JSON without being recast as a shell result.
  assert.deepEqual(nativeItemToAgentMessages({ ...envelope, item: { ...envelope.item, origin: undefined } })[0].content, envelope.item.content);
  assert.deepEqual(nativeItemToAgentMessages(hostItem(scheduleOrigin, raw))[0].content, envelope.item.content);
});

test("background shell display keeps failures and ignores unrelated JSON", () => {
  const failed = JSON.stringify({ status: "ok", result: { handle_id: "handle-1", running: false,
    command: "run check", exit_code: 7, output: "diagnostic", error: "diagnostic" } });
  const entry = hostQueueEntry(shellOrigin, failed);
  assert.equal(queueSelectionText(entry), "Shell: failed · exit 7 · run check\ndiagnostic");
  const canceled = JSON.stringify({ outcome: "terminal", task: { kind: "command", state: "canceled",
    command: { command: "long check" } }, output: "Command canceled" });
  assert.equal(queueSelectionText(hostQueueEntry(shellOrigin, canceled)), "Shell: canceled · long check\nCommand canceled");
  const exec = JSON.stringify({ outcome: "terminal", task: { kind: "command", state: "completed",
    command: { command: "printf done", exit_code: 0 } }, output: "Process exited with code 0\ndone" });
  assert.equal(queueSelectionText(hostQueueEntry(shellOrigin, exec)), "Shell: exit 0 · printf done\ndone");
  const other = '{"status":"ok","result":{"command":"not a shell result"}}';
  assert.equal(queueSelectionText(hostQueueEntry(shellOrigin, other)), `Background: ${other}`);
  assert.deepEqual(nativeItemToAgentMessages(hostItem(shellOrigin, other))[0].content, [{ type: "text", text: other }]);
});

for (const status of ["waiting_for_input", "completed", "failed", "interrupted", "canceled", "closed"]) {
  test(`automatic child ${status} completion is omitted from live and replay transcript projection`, () => {
    const origin = { source: "agent", sender: "ses_child", senderName: "Reviewer", eventId: `child:ses_child:turn_1:${status}`, delivery: "followUp" };
    const text = `[system] Child agent /root/reviewer (ses_child) ${status}.\nUseful result`;
    const envelope = { createdAt: "2026-11-09T00:00:00Z", item: { type: "userMessage", origin, content: [{ type: "text", text }] } };
    assert.deepEqual(nativeItemToAgentMessages(envelope), []);
    assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), []);
    assert.equal(envelope.item.content[0].text, text, "projection must not mutate durable/model text");
  });
}

test("system-looking user and explicit agent messages are never stripped", () => {
  const text = "[system] Child agent /root/reviewer (ses_child) completed.\nActual quoted message";
  for (const origin of [undefined, { source: "agent", sender: "ses_child", eventId: "explicit-message" }, { source: "agent", sender: "other", eventId: "child:ses_child:turn_1:completed" }, { source: "backgroundTask", sender: "ses_child", eventId: "child:ses_child:turn_1:completed" }]) {
    const result = nativeItemToAgentMessages({ item: { type: "userMessage", content: [{ type: "text", text }], ...(origin ? { origin } : {}) } });
    assert.ok(JSON.stringify(result).includes(JSON.stringify(text).slice(1, -1)));
  }
});

test("explicit inter-agent messages remain visible even when they resemble child completion text", () => {
  const origin = { source: "agent", sender: "ses_child", eventId: "explicit-message" };
  const text = "[system] Child agent /root/reviewer (ses_child) failed.\nAdditional diagnostic";
  const [result] = nativeItemToAgentMessages({ item: { type: "userMessage", origin, content: [{ type: "text", text }] } });
  assert.deepEqual(result.content, [{ type: "text", text: "From: ses_child" }, { type: "text", text }]);
});
