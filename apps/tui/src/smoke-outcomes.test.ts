import assert from "node:assert/strict";
import { test } from "node:test";
import { heartbeatDelivered, rpcSucceeded, turnOutcome } from "../scripts/smoke-outcomes.js";

const item = (type: string, extra = {}, turnId = "turn-1") => ({ method: "item/completed",
  payload: { params: { item: { turnId, item: { type, ...extra } } } } });
const started = { method: "turn/started", payload: { params: { turn: { id: "turn-1" } } } };
const ended = (status: string) => ({ method: "turn/completed",
  payload: { params: { turn: { id: "turn-1", status } } } });

test("smoke requires a successful response to the matching session request", () => {
  const request = { id: 2, direction: "tui-to-server", kind: "request", method: "session/new" };
  const response = { id: 2, direction: "server-to-tui", kind: "response", payload: { result: {} } };
  assert.equal(rpcSucceeded([request], "session/new"), false);
  assert.equal(rpcSucceeded([request, { ...response, id: 3 }], "session/new"), false);
  assert.equal(rpcSucceeded([request, { ...response, payload: { error: {} } }], "session/new"), false);
  assert.equal(rpcSucceeded([request, response], "session/new"), true);
});

test("smoke cannot pass a prompt echo or failed/empty turn", () => {
  const events = [started, item("userMessage"), ended("completed")];
  assert.equal(turnOutcome(events, "completed"), false);
  assert.equal(turnOutcome([...events, item("assistantMessage", { text: "hello" }, "other-turn")], "completed"), false);
  assert.equal(turnOutcome([started, item("userMessage"), item("assistantMessage", { text: "hello" }), ended("failed")], "completed"), false);
  assert.equal(turnOutcome([...events, item("assistantMessage", { text: "hello" })], "completed"), true);
});

test("smoke interruption must terminate the started user turn", () => {
  assert.equal(turnOutcome([started, item("userMessage")], "interrupted"), false);
  assert.equal(turnOutcome([started, item("userMessage"), ended("completed")], "interrupted"), false);
  assert.equal(turnOutcome([started, item("userMessage"), ended("interrupted")], "interrupted"), true);
});

test("heartbeat slash echo cannot stand in for scheduler delivery", () => {
  const marker = "heartbeat-probe";
  const content = [{ type: "text", text: marker }];
  assert.equal(heartbeatDelivered([{ method: "turn/start", payload: { params: { input: marker } } }], marker), false);
  assert.equal(heartbeatDelivered([item("assistantMessage", { content })], marker), false);
  assert.equal(heartbeatDelivered([item("userMessage", { content })], marker), true);
});
