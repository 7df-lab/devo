import assert from "node:assert/strict";
import { test } from "node:test";
import { queuedTurnCompleted } from "./queue-output-smoke.mjs";
function fixture(mode, turnId = mode === "steer" ? "initial" : "next") {
  return [
    { method: "item/started", payload: { params: { item: { id: "user", turnId, sessionId: "s", item: {
      type: "userMessage", content: [{ type: "text", text: `queue-output queued ${mode}` }],
    } } } } },
    { method: "item/completed", payload: { params: { item: { id: "reply", turnId, sessionId: "s", state: "completed", item: {
      type: "assistantMessage", text: `QUEUED_REPLY_${mode.toUpperCase()}`,
    } } } } },
    { method: "turn/completed", payload: { params: { turn: { id: turnId, sessionId: "s", status: "completed" } } } },
  ];
}
test("follow-up and steering bind the exact admission/reply/completed turn", () => {
  for (const mode of ["follow", "steer"]) assert.equal(queuedTurnCompleted(fixture(mode), mode, "initial"), true);
});
test("streamed reply or initial completion cannot stand in for queued completion", () => {
  const events = fixture("follow");
  events[2].payload.params.turn.id = "initial";
  assert.equal(queuedTurnCompleted(events, "follow", "initial"), false);
  events[1].method = "item/agentMessage/delta";
  assert.equal(queuedTurnCompleted(events, "follow", "initial"), false);
});
test("steer must remain in active turn; follow-up must start a different turn", () => {
  assert.equal(queuedTurnCompleted(fixture("steer", "next"), "steer", "initial"), false);
  assert.equal(queuedTurnCompleted(fixture("follow", "initial"), "follow", "initial"), false);
});
test("wrong session, reply turn, duplicate reply and premature completion are rejected", () => {
  for (const mutate of [
    events => { events[1].payload.params.item.sessionId = "other"; },
    events => { events[1].payload.params.item.turnId = "other"; },
    events => { events.push(events[1]); },
    events => { events.unshift(events.pop()); },
  ]) {
    const events = fixture("follow"); mutate(events);
    assert.equal(queuedTurnCompleted(events, "follow", "initial"), false);
  }
});
