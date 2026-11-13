import { test } from "node:test";
import assert from "node:assert/strict";
import { assertInterruptedHistory } from "../scripts/abort-history-outcome.mjs";
const turns = [{ id: "first", sequence: 1, status: "interrupted" }, { id: "queued", sequence: 2, status: "interrupted" }];
const items = turns.flatMap(turn => [
  { turnId: turn.id, item: { type: "userMessage", content: [{ type: "text", text: turn.id }] } },
  { turnId: turn.id, item: { type: "assistantMessage", text: "ABORT_PARTIAL_MARKER" } },
]);
const records = ["session/turns/list", "session/items/list"].flatMap((method, id) => [
  { direction: "tui-to-server", method, id, payload: { params: { sessionId: "session" } } },
  { direction: "server-to-tui", kind: "response", id, payload: { result: { data: id === 0 ? turns : items, nextCursor: null } } },
]);
const screen = "first\nABORT_PARTIAL_MARKER\nOperation aborted\nqueued\nABORT_PARTIAL_MARKER\nOperation aborted\n";
test("two durable interrupted turns each own one marker", () => assert.equal(assertInterruptedHistory(screen, records, "session", "first"), 2));
test("duplicate abort in one cell is rejected even if total equals durable turns", () => {
  assert.throws(() => assertInterruptedHistory(screen.replace("Operation aborted\nqueued", "Operation aborted\nOperation aborted\nqueued").replace(/Operation aborted\n$/, ""), records, "session", "first"));
});
test("missing original abort, repeated reply and reversed turn order are rejected", () => {
  for (const bad of [screen.replace("Operation aborted\n", ""), screen.replace("first\n", "first\nABORT_PARTIAL_MARKER\n"), screen.replace("first", "TEMP").replace("queued", "first").replace("TEMP", "queued")]) {
    assert.throws(() => assertInterruptedHistory(bad, records, "session", "first"));
  }
});
test("only authoritative original-session pages can satisfy the oracle", () => {
  assert.throws(() => assertInterruptedHistory(screen, records, "other", "first"));
  assert.throws(() => assertInterruptedHistory(screen, records, "session", "unknown"));
});
