import { test } from "node:test";
import assert from "node:assert/strict";
import { nativeItemsToAgentMessages } from "./messages.js";
import { applyInterruptedTurnsToAgentMessages } from "./interrupted-history.js";
const item = (turnId: string, type: string, text: string) => ({ turnId, item: { type, text, content: [{ type: "text", text }] } });
const turn = (id: string, sequence: number, status = "interrupted") => ({ id, sequence, status, completedAt: "2026-01-01T00:00:01Z" });
const labels = (messages: Record<string, unknown>[]) => messages.map(m => m.stopReason === "aborted" ? "ABORT" : (m.content as {text: string}[])[0]?.text);
test("interrupted history restores a marker per owning turn, not on the latest reply", () => {
  const original = nativeItemsToAgentMessages([
    item("a", "userMessage", "first"), item("a", "assistantMessage", "partial"),
    item("b", "userMessage", "second"), item("b", "assistantMessage", "done"),
    item("c", "userMessage", "third"),
  ]);
  const turns = [turn("c", 3), turn("b", 2, "completed"), turn("a", 1), turn("a", 1)];
  const restored = applyInterruptedTurnsToAgentMessages(original, turns);
  assert.deepEqual(labels(restored), ["first", "partial", "ABORT", "second", "done", "third", "ABORT"]);
  assert.deepEqual(applyInterruptedTurnsToAgentMessages(restored, turns), restored);
  assert.equal(original.length, 5);
  assert.equal(restored[4].stopReason, null);
});
test("pre-output interruption and empty turns retain sequence before a later turn", () => {
  const messages = nativeItemsToAgentMessages([item("b", "userMessage", "next")]);
  assert.deepEqual(labels(applyInterruptedTurnsToAgentMessages(messages, [turn("b", 2, "inProgress"), turn("a", 1)])), ["ABORT", "next"]);
  assert.deepEqual(labels(applyInterruptedTurnsToAgentMessages([], [turn("a", 1)])), ["ABORT"]);
});
test("non-interrupted turns and malformed input never invent aborts", () => {
  const messages = nativeItemsToAgentMessages([item("a", "userMessage", "hello")]);
  assert.equal(applyInterruptedTurnsToAgentMessages(messages, [null, {}, turn("a", 1, "failed"), turn("b", 2, "completed")]), messages);
});
test("tool and partial reply histories retain one terminal abort without relabelling tools", () => {
  const messages = nativeItemsToAgentMessages([
    item("a", "userMessage", "tool"),
    { turnId: "a", item: { type: "toolCall", callId: "call", toolName: "ipython", arguments: { code: "1" } } },
    { turnId: "a", item: { type: "toolResult", callId: "call", toolName: "ipython", output: { content: [{ type: "text", text: "1" }] } } },
    item("b", "userMessage", "later"),
  ]);
  const restored = applyInterruptedTurnsToAgentMessages(messages, [turn("a", 1), turn("b", 2, "completed")]);
  assert.deepEqual(restored.map(message => [message.role, message.stopReason ?? null]), [
    ["user", null], ["assistant", "toolUse"], ["toolResult", null], ["assistant", "aborted"], ["user", null],
  ]);
});
test("mixed failed and interrupted turns never annotate an earlier reply", async () => {
  const { applyFailedTurnsToAgentMessages } = await import("./messages.js");
  const messages = nativeItemsToAgentMessages([
    item("a", "userMessage", "first"), item("a", "assistantMessage", "partial"),
    item("b", "userMessage", "failed request"), item("c", "userMessage", "later"),
  ]);
  const turns = [turn("a", 1), { ...turn("b", 2, "failed"), error: { message: "network" } }, turn("c", 3, "completed")];
  const restored = applyFailedTurnsToAgentMessages(applyInterruptedTurnsToAgentMessages(messages, turns), turns);
  assert.deepEqual(labels(restored), ["first", "partial", "ABORT", "failed request", "", "later"]);
  assert.equal(restored[1].stopReason, null);
  assert.equal(restored[4].errorMessage, "network");
  assert.deepEqual(applyFailedTurnsToAgentMessages(restored, turns), restored);
});
test("synthetic orphan pairs and interrupted tool results stay with their turn", () => {
  const orphan = nativeItemsToAgentMessages([{ turnId: "b", item: { type: "toolResult", callId: "orphan", toolName: "ipython", output: "ok" } }]);
  assert.deepEqual(applyInterruptedTurnsToAgentMessages(orphan, [turn("a", 1), turn("b", 2, "completed")]).map(m => [m.role, m.stopReason ?? null]), [["assistant", "aborted"], ["assistant", "toolUse"], ["toolResult", null]]);
  const pending = nativeItemsToAgentMessages([{ turnId: "a", item: { type: "toolCall", callId: "pending", toolName: "ipython", arguments: {} } }, item("b", "userMessage", "later")]);
  assert.deepEqual(applyInterruptedTurnsToAgentMessages(pending, [turn("a", 1), turn("b", 2, "completed")]).map(m => [m.role, m.stopReason ?? null]), [["assistant", "toolUse"], ["toolResult", null], ["assistant", "aborted"], ["user", null]]);
});
test("missing-item failed and interrupted turns retain durable sequence", async () => {
  const { applyFailedTurnsToAgentMessages } = await import("./messages.js");
  const turns = [{ ...turn("a", 1, "failed"), error: { message: "failed first" } }, turn("b", 2)];
  const restored = applyFailedTurnsToAgentMessages(applyInterruptedTurnsToAgentMessages([], turns), turns);
  assert.deepEqual(restored.map(message => message.stopReason), ["error", "aborted"]);
});
