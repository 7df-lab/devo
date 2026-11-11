import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";
import { QueueSelection } from "../lib/coding-agent/src/modes/interactive/queue-selection.js";

const entries = [
  { queueItemId: "a", position: 0, delivery: "steer", preview: "first", input: [{ type: "text", text: "first" }] },
  { queueItemId: "b", position: 1, delivery: "followUp", preview: "second", input: [{ type: "text", text: "second" }] },
  { queueItemId: "c", position: 2, delivery: "steer", preview: "third", input: [{ type: "text", text: "third" }] },
];
const ordered = [
  { id: "a", text: "first", delivery: "steer" as const, blocked: false },
  { id: "b", text: "second", delivery: "followUp" as const, blocked: false },
  { id: "c", text: "third", delivery: "steer" as const, blocked: true },
];

test("queue notifications retain mixed delivery order and show the follow-up barrier", () => {
  assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [...entries].reverse() }, createStreamState()), [
    { type: "session_action_update", actions: { queuedCount: 3, steering: ["first", "third"], followUps: ["second"], ordered } },
  ]);
});

test("queue browsing follows admission order instead of regrouping delivery lanes", () => {
  const selection = new QueueSelection();
  const queue = { steering: ["first", "third"], followUp: ["second"], ordered };
  assert.equal(selection.move(queue, "draft", -1), "third");
  assert.deepEqual(selection.selected, { id: "c", lane: "steering", index: 1, text: "third" });
  assert.equal(selection.refreshAt(queue, "steering", 1, "third"), undefined);
  assert.equal(selection.move(queue, "third", -1), "second");
  assert.equal(selection.move(queue, "second", -1), "first");
});

test("accepted steering remains visible until server consumption; editing never removes and requeues", async () => {
  let conn: NativeAgentConnection;
  const requests: { method: string; params: Record<string, unknown> }[] = [];
  conn = NativeAgentConnection.create({ writeLine: line => {
    const request = JSON.parse(line);
    if (request.id == null) return;
    requests.push(request);
    const result = request.method === "session/new" ? { session: { id: "root", title: "Root" } }
      : request.method === "subscription/create" ? { subscriptionId: "sub", snapshots: [], replay: [] }
      : request.method === "session/items/list" ? { data: [], nextCursor: null }
      : request.method === "session/queue/list" ? { entries }
      : request.method === "turn/steer" ? { outcome: "queued", entry: entries[2] } : {};
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
  } });
  try {
    await conn.newSession();
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "turn/started", params: { turn: { id: "turn", sessionId: "root" } } }) + "\n");
    await conn.steer("third");
    assert.deepEqual(await conn.getQueue(), { steering: ["first", "third"], followUp: ["second"], ordered });
    assert.equal(await conn.mutateQueuedMessage("steering", 1, "third", { type: "move", direction: -1 }), "rejected");
    assert.equal(await conn.mutateQueuedMessage("steering", 1, "third", { type: "delete", queueItemId: "already-consumed" }), "rejected");
    assert.equal(await conn.mutateQueuedMessage("steering", 1, "third", { type: "replace", text: "edit third", lane: "steering" }), "applied");
    const update = requests.find(r => r.method === "session/queue/update");
    assert.equal(update?.params.queueItemId, "c");
    assert.equal(requests.some(r => r.method === "session/queue/remove"), false);
  } finally {
    await conn.dispose();
  }
});

test("a late queue read cannot resurrect input after a newer consumption event", async () => {
  let conn: NativeAgentConnection;
  let deferQueue = false;
  let finishQueue: (() => void) | undefined;
  conn = NativeAgentConnection.create({ writeLine: line => {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/queue/list" && deferQueue) {
      finishQueue = () => respond({ entries });
      return;
    }
    respond(request.method === "session/new" ? { session: { id: "root", title: "Root" } }
      : request.method === "subscription/create" ? { subscriptionId: "sub", snapshots: [], replay: [] }
      : request.method === "session/items/list" ? { data: [], nextCursor: null }
      : request.method === "session/queue/list" ? { entries: [] } : {});
  } });
  try {
    await conn.newSession();
    deferQueue = true;
    const submit = conn.followUp("second");
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.ok(finishQueue);
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "queue/updated", params: { sessionId: "root", queue: [] } }) + "\n");
    finishQueue();
    await submit;
    assert.deepEqual(await conn.getQueue(), { steering: [], followUp: [], ordered: [] });
  } finally { await conn.dispose(); }
});

test("a versioned late read cannot overwrite a newer server queue revision", async () => {
  let conn: NativeAgentConnection;
  let deferQueue = false;
  let finishQueue: (() => void) | undefined;
  conn = NativeAgentConnection.create({ writeLine: line => {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    if (request.method === "session/queue/list" && deferQueue) {
      finishQueue = () => respond({ entries, revision: 2 });
      return;
    }
    respond(request.method === "session/new" ? { session: { id: "root", title: "Root" } }
      : request.method === "subscription/create" ? { subscriptionId: "sub", snapshots: [], replay: [] }
      : request.method === "session/items/list" ? { data: [], nextCursor: null }
      : request.method === "session/queue/list" ? { entries: [] } : {});
  } });
  try {
    await conn.newSession();
    deferQueue = true;
    const submit = conn.followUp("second");
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.ok(finishQueue);
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "queue/updated", params: { sessionId: "root", revision: 3, queue: [] } }) + "\n");
    finishQueue();
    await submit;
    assert.deepEqual(await conn.getQueue(), { steering: [], followUp: [], ordered: [] });
  } finally { await conn.dispose(); }
});
