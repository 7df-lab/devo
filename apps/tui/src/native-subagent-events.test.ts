import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { nativeNotificationSessionId } from "./native-notification-session.js";
import { hostMessageForkText, queuePreviewText, queueSelectionText } from "./native-connection-ops.js";
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

test("routine spawn cards are omitted from live and replay transcript projection", () => {
  assert.deepEqual(nativeItemsToAgentMessages([spawn]), []);
  assert.deepEqual(nativeNotificationToPiEvents("item/started", { item: spawn }, createStreamState()), []);
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
  assert.equal(hostMessageForkText(origin, "Result"), "[Agent message · reviewer] Result");
  assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [{ origin, preview: "Result" }] }, createStreamState()), [
    { type: "session_action_update", actions: { queuedCount: 1, steering: [], followUps: ["reviewer: Result"], ordered: [{ id: "", text: "reviewer: Result", delivery: "followUp", blocked: false, editable: false }] } },
  ]);
});

const resultOrigin = { source: "agent", sender: "ses_child", senderName: "Reviewer", eventId: "child:ses_child:turn_1:completed", delivery: "steer" };
const resultPrefix = "Reviewer: Useful result";
const resultText = "[system] Child agent /root/reviewer (ses_child) completed.\nUseful result\nMore detail";
const completion = { id: "result", sessionId: "root", turnId: "root-turn", state: "completed", item: {
  type: "userMessage", entry: "steer", origin: resultOrigin, content: [{ type: "text", text: resultText }],
} };

test("agent queue previews use the real body and browsing keeps all user-visible text", () => {
  const entry = { queueItemId: "opaque-queue-id", position: 0, preview: "[system] Child agent /root/revi…", origin: resultOrigin };
  assert.equal(queuePreviewText(entry), "Reviewer");
  assert.equal(queuePreviewText({ ...entry, origin: { ...resultOrigin, eventId: "child:ses_child:turn_1:failed" } }), "Reviewer: failed");
  const withInput = { ...entry, input: completion.item.content };
  assert.equal(queuePreviewText(withInput), resultPrefix);
  assert.equal(queueSelectionText(withInput), `${resultPrefix}\nMore detail`);
  const directed = { ...entry, origin: { ...resultOrigin, eventId: "direct-message" },
    preview: "shortened", input: [{ type: "text", text: "\u001b[2JDirect question\nSecond line" }] };
  assert.equal(queuePreviewText(directed), "Reviewer: Direct question");
  assert.equal(queueSelectionText(directed), "Reviewer: Direct question\nSecond line");
  const ordinary = { queueItemId: "ordinary", position: 0, preview: "A regular queued note" };
  assert.equal(queuePreviewText(ordinary), "A regular queued note");
  assert.equal(queueSelectionText(ordinary), "A regular queued note");
});

test("child results are visible only while queued, not again on delivery", async () => {
  const conn = await connection();
  const events: unknown[] = [];
  conn.subscribe(event => { if (event.type === "session_event") events.push(event.event); });
  const queue = [{ queueItemId: "result-queue", delivery: "steer", origin: resultOrigin, input: completion.item.content, preview: "[system] Child agent" }];
  try {
    push(conn, "queue/updated", { sessionId: "root", queue });
    push(conn, "item/started", { item: completion });
    push(conn, "item/completed", { item: completion });
    push(conn, "queue/updated", { sessionId: "root", queue: [] });
    assert.deepEqual(events, [
      { type: "session_action_update", actions: { queuedCount: 1, steering: [`${resultPrefix}\nMore detail`], followUps: [], ordered: [
        { id: "result-queue", text: `${resultPrefix}\nMore detail`, preview: resultPrefix, delivery: "steer", blocked: false, editable: false },
      ] } },
      { type: "session_action_update", actions: { queuedCount: 0, steering: [], followUps: [], ordered: [] } },
    ]);
  } finally {
    await conn.dispose();
  }
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
    assert.equal(root.length, 0);
    assert.equal(child.filter(event => (event.message as Record<string, unknown>)?.customType === "Agent spawned").length, 0);
  } finally {
    await watcher.close();
    await conn.dispose();
  }
});

test("subscription replay keeps child results and child-session envelopes out of the root transcript", async () => {
  const replay = [
    { notification: { method: "item/started", params: { item: childInput } } },
    { notification: { method: "item/completed", params: { item: kernel } } },
    { notification: { method: "item/completed", params: { item: completion } } },
  ];
  const conn = await connection(replay);
  const events: unknown[] = [];
  conn.subscribe(event => { if (event.type === "session_event") events.push(event.event); });
  await conn.newSession();
  assert.deepEqual(events.filter(event => (event as { type: string }).type !== "session_action_update"), []);
  await conn.dispose();
});
