import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { nativeNotificationSessionId } from "./native-notification-session.js";
import { hostMessagePrefix } from "./native-connection-ops.js";
import { nativeItemsToAgentMessages } from "./native-to-pi/messages.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";

const timestamp = "2026-11-11T00:00:00Z";
const childInput = { id: "child-input", sessionId: "child", createdAt: timestamp, item: {
  type: "userMessage", origin: { source: "agent", delivery: "followUp", sender: "root", senderName: "Main agent", eventId: "startup" },
  content: [{ type: "text", text: "Inspect the code" }],
} };
const kernel = { id: "kernel", sessionId: "child", createdAt: timestamp, item: {
  type: "warning", code: "ipythonKernelStateFresh", message: "Started fresh Python kernel",
} };
const spawn = { id: "spawn", sessionId: "root", createdAt: timestamp, item: {
  type: "subAgent", agentSessionId: "child", parentSessionId: "root", agentNickname: "reviewer", role: "default", task: "Inspect the code", state: "running",
} };

async function connection(replay: unknown[] = []) {
  let conn: NativeAgentConnection;
  conn = NativeAgentConnection.create({ writeLine: line => {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const result = request.method === "session/new" ? { session: { id: "root", title: "Root" } }
      : request.method === "subscription/create" ? { subscriptionId: "sub", snapshots: [], replay }
      : request.method === "session/items/list" ? { data: [], nextCursor: null } : {};
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  await conn.newSession();
  return conn;
}

function push(conn: NativeAgentConnection, method: string, params: unknown) {
  conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
}

for (const params of [
  { sessionId: "child" }, { session_id: "child" }, { item: { sessionId: "child" } },
  { item: { session_id: "child" } }, { turn: { sessionId: "child" } },
  { goal: { sessionId: "child" } }, { request: { sessionId: "child" } }, { session: { id: "child" } },
]) {
  test(`Native owner resolves ${JSON.stringify(params)}`, () => assert.equal(nativeNotificationSessionId(params), "child"));
}

test("spawn has a named outgoing row and identical live/replay semantics", () => {
  const messages = nativeItemsToAgentMessages([spawn]);
  assert.deepEqual(messages, [{ role: "custom", customType: "Agent spawned", display: true,
    content: [{ type: "text", text: "Spawned reviewer" }, { type: "text", text: "Inspect the code" }],
    details: { agentSessionId: "child" }, timestamp: Date.parse(timestamp),
  }]);
  assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: spawn }, createStreamState()), [
    { type: "message_start", message: messages[0] }, { type: "message_end", message: messages[0] },
  ]);
});

test("named incoming sender keeps stable identity in live, replay and queue labels", () => {
  const origin = { source: "agent", delivery: "followUp", eventId: "result", sender: "ses_opaque", senderName: "reviewer" };
  const envelope = { ...childInput, sessionId: "root", item: { ...childInput.item, origin } };
  const [message] = nativeItemsToAgentMessages([envelope]);
  assert.deepEqual(message, { role: "custom", customType: "Agent message", display: true, details: { origin },
    content: [{ type: "text", text: "From: reviewer" }, { type: "text", text: "Inspect the code" }], timestamp: Date.parse(timestamp) });
  assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState()), [
    { type: "message_start", message }, { type: "message_end", message },
  ]);
  assert.equal(hostMessagePrefix(origin), "[Agent message · reviewer] ");
  assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [{ origin, preview: "Result" }] }, createStreamState()), [
    { type: "session_action_update", actions: { queuedCount: 1, steering: [], followUps: ["[Agent message · reviewer] Result"] } },
  ]);
});

test("child input, kernel, lifecycle and stream events reach only its monitor", async () => {
  const conn = await connection();
  const root: unknown[] = [];
  const child: Array<Record<string, unknown>> = [];
  conn.subscribe(event => { if (event.type === "session_event") root.push(event.event); });
  const watcher = await conn.watchSession("child");
  assert.ok(watcher);
  watcher.subscribe(event => { if (event.type === "session_event") child.push(event.event as unknown as Record<string, unknown>); });
  try {
    push(conn, "turn/started", { turn: { id: "child-turn", sessionId: "child" } });
    push(conn, "item/started", { item: childInput });
    push(conn, "item/completed", { item: kernel });
    push(conn, "item/assistantMessage/delta", { sessionId: "child", itemId: "answer", delta: "Hello " });
    push(conn, "item/assistantMessage/delta", { sessionId: "child", itemId: "answer", delta: "world" });
    push(conn, "item/completed", { item: { sessionId: "child", id: "answer", item: { type: "assistantMessage", text: "Hello world" } } });
    push(conn, "turn/completed", { turn: { id: "child-turn", sessionId: "child", status: "completed" } });
    assert.deepEqual(root, []);
    assert.equal((await conn.getState()).isStreaming, false);
    assert.ok(child.some(event => event.type === "message_start" && (event.message as Record<string, unknown>)?.customType === "Agent message"));
    assert.deepEqual(child.filter(event => event.type === "message_update").map(event => (event.message as { content: unknown[] }).content), [
      [{ type: "text", text: "Hello " }], [{ type: "text", text: "Hello world" }],
    ]);
    push(conn, "item/started", { item: spawn });
    assert.equal(root.length, 2);
    assert.equal(child.filter(event => (event.message as Record<string, unknown>)?.customType === "Agent spawned").length, 0);
  } finally {
    await watcher.close();
    await conn.dispose();
  }
});

test("subscription replay cannot leak child envelopes into a newly selected root", async () => {
  const replay = [
    { notification: { method: "item/started", params: { item: childInput } } },
    { notification: { method: "item/completed", params: { item: kernel } } },
  ];
  const conn = await connection(replay);
  const events: unknown[] = [];
  conn.subscribe(event => { if (event.type === "session_event") events.push(event.event); });
  await conn.newSession();
  assert.deepEqual(events.filter(event => (event as { type: string }).type !== "session_action_update"), []);
  await conn.dispose();
});
