import assert from "node:assert/strict";
import { test } from "node:test";
import { completedItemRevisions, notificationItemIdentity } from "./native-history-hydration.js";

test("completed snapshots cover stable item revision but never running or malformed entries", () => {
  assert.deepEqual({ ...completedItemRevisions([
    { id: "done", revision: 2, state: "completed" },
    { id: "done", revision: 1, state: "completed" },
    { id: "running", revision: 1, state: "running" },
    { id: "waiting", revision: 1, state: "waiting" },
    { id: "failed", revision: 3, state: "failed" },
    { id: "legacy" }, null,
  ]) }, { done: 2, failed: 3 });
});

test("notification metadata extracts item lifecycle and delta IDs but not foreign event identifiers", () => {
  assert.deepEqual(notificationItemIdentity("item/completed", { item: { id: "x", revision: 2 } }), { id: "x", revision: 2 });
  assert.deepEqual(notificationItemIdentity("item/assistantMessage/delta", { itemId: "x" }), { id: "x" });
  assert.equal(notificationItemIdentity("turn/completed", { itemId: "x" }), undefined);
  assert.equal(notificationItemIdentity("item/started", {}), undefined);
});

import { NativeAgentConnection } from "./native-agent-connection.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";

test("Native snapshot keeps running text out of finalized history and returns its stable identity", async () => {
  let connection: NativeAgentConnection;
  const old = { id: "old", seq: 1, revision: 1, state: "completed", createdAt: "2026-11-11T00:00:00Z", item: { type: "userMessage", content: [{ type: "text", text: "old" }] } };
  const partial = { id: "active", seq: 2, revision: 1, state: "running", createdAt: "2026-11-11T00:00:01Z", item: { type: "assistantMessage", text: "prefix" } };
  connection = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const result = request.method === "session/items/list" ? { data: [old, partial], nextCursor: null }
      : request.method === "session/turns/list" ? { data: [], nextCursor: null } : {};
    connection.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  Object.assign(connection, { sessionId: "child", activeSessionId: "child" });
  try {
    const snapshot = await connection.getInitialSnapshot();
    assert.equal(snapshot.messages.length, 1);
    assert.deepEqual(snapshot.streamingItemIds, ["active"]);
    assert.deepEqual((snapshot.streamingMessage as { content?: unknown } | undefined)?.content, [{ type: "text", text: "prefix" }]);
    assert.deepEqual({ ...snapshot.completedItemRevisions }, { old: 1 });
  } finally { await connection.dispose(); }
});

test("mid-turn attach uses canonical completed text, not the observed delta suffix", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents("item/assistantMessage/delta", { itemId: "active", delta: "suffix" }, state);
  const result = nativeNotificationToPiEvents("item/completed", { item: { id: "active", item: { type: "assistantMessage", text: "prefix suffix" } } }, state);
  assert.deepEqual((result[0].message as { content: unknown }).content, [{ type: "text", text: "prefix suffix" }]);
});

import { normalizeHistoryItems } from "./native-history-hydration.js";
test("overlapping pages retain highest revision and canonical sequence, not arrival order", () => {
  const first = { id: "old", seq: 1, revision: 2, text: "latest" };
  const second = { id: "new", seq: 2, revision: 1 };
  assert.deepEqual(normalizeHistoryItems([second, first, { ...first, revision: 1, text: "stale" }, second]), [first, second]);
  const legacy = [{ item: { type: "userMessage" } }, { item: { type: "assistantMessage" } }];
  assert.deepEqual(normalizeHistoryItems(legacy), legacy);
});

test("history birth prefix seeds live accumulators before the next delta", async () => {
  let conn: NativeAgentConnection;
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line); if (request.id == null) return;
    const result = request.method === "session/items/list" ? { data: [{ id: "active", seq: 1, revision: 1, state: "running", item: { type: "assistantMessage", text: "prefix " } }] }
      : request.method === "session/turns/list" ? { data: [] } : {};
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  Object.assign(conn, { sessionId: "child" });
  const events: any[] = []; conn.subscribe(event => { events.push(event); });
  try {
    await conn.getInitialSnapshot();
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "item/assistantMessage/delta", params: { sessionId: "child", itemId: "active", delta: "suffix" } }) + "\n");
    const update = events.find(event => event.event?.type === "message_update");
    assert.deepEqual(update.event.message.content, [{ type: "text", text: "prefix suffix" }]);
    assert.deepEqual(update.sourceItem.missingTextBaseIds, []);
  } finally { await conn.dispose(); }
});

test("reasoning completion replaces a mid-turn suffix with canonical full thinking", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents("item/reasoning/delta", { itemId: "reason", delta: "suffix" }, state);
  const events = nativeNotificationToPiEvents("item/completed", { item: { id: "reason", item: { type: "reasoning", text: "prefix suffix" } } }, state);
  assert.deepEqual((events[0].message as { content: unknown }).content, [{ type: "thinking", thinking: "prefix suffix" }, { type: "text", text: "" }]);
});

import { hydrateQueuedTextEvent } from "../lib/coding-agent/src/modes/interactive/transcript-hydration.js";
test("mixed thinking/text queued updates and completion repair each independently missing birth prefix", async () => {
  let conn: NativeAgentConnection;
  const items = [
    { id: "r", seq: 1, revision: 1, state: "running", item: { type: "reasoning", text: "Rbase " } },
    { id: "a", seq: 2, revision: 1, state: "running", item: { type: "assistantMessage", text: "Abase " } },
  ];
  conn = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line); if (request.id == null) return;
    const result = request.method === "session/items/list" ? { data: items } : request.method === "session/turns/list" ? { data: [] } : {};
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  Object.assign(conn, { sessionId: "child" });
  const events: any[] = []; conn.subscribe(event => { events.push(event); });
  const push = (method: string, params: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
  try {
    push("item/reasoning/delta", { sessionId: "child", itemId: "r", delta: "Rtail" });
    push("item/assistantMessage/delta", { sessionId: "child", itemId: "a", delta: "Atail" });
    push("item/completed", { item: { id: "a", sessionId: "child", item: { type: "assistantMessage", text: "Abase Atail" } } });
    const snapshot = await conn.getInitialSnapshot();
    const projected = events.filter(event => event.type === "session_event" && ["message_update", "message_end"].includes(event.event.type))
      .map(event => hydrateQueuedTextEvent(event, snapshot.streamingPrefixes ?? {}) as any);
    const expected = [{ type: "thinking", thinking: "Rbase Rtail" }, { type: "text", text: "Abase Atail" }];
    assert.deepEqual(projected.at(-2).message.content, expected);
    assert.deepEqual(projected.at(-1).message.content, expected);
    const priorEnds = events.filter(event => event.event?.type === "message_end").length;
    push("item/started", { item: { id: "tool", sessionId: "child", item: { type: "toolCall", callId: "call", toolName: "read", arguments: {} } } });
    assert.equal(events.filter(event => event.event?.type === "message_end").length, priorEnds, "stale snapshot must not resurrect closed reasoning");
  } finally { await conn.dispose(); }
});

test("snapshot fences turn lifecycle notifications by identity without suppressing future terminal events", async () => {
  let connection: NativeAgentConnection;
  connection = NativeAgentConnection.create({ writeLine(line) {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const result = request.method === "session/turns/list"
      ? { data: [{ id: "compact", status: "completed" }, { id: "followup", status: "inProgress" }], nextCursor: null }
      : request.method === "session/items/list" ? { data: [], nextCursor: null } : {};
    connection.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  Object.assign(connection, { sessionId: "child", activeSessionId: "child" });
  const events: any[] = [];
  connection.subscribe(event => { events.push(event); });
  try {
    const snapshot = await connection.getInitialSnapshot();
    assert.deepEqual(snapshot.transcriptTurns, { compact: "terminal", followup: "started" });
    connection.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "turn/started", params: {
      sessionId: "child", turn: { id: "followup", sessionId: "child", status: "inProgress" },
    } }) + "\n");
    assert.deepEqual(events.find(event => event.event?.type === "agent_start")?.sourceTurn,
      { id: "followup", phase: "started" });
  } finally { await connection.dispose(); }
});
