import assert from "node:assert/strict";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";

const entry = (id: string, delivery = "followUp") => ({ queueItemId: id, position: 0, preview: id, delivery, input: [{ type: "text", text: id }] });

async function harness() {
  let conn: NativeAgentConnection;
  const state = {
    entries: [entry("first"), { ...entry("second", "steer"), position: 1 }] as Array<Record<string, unknown>>,
    updates: [] as Record<string, unknown>[],
    failList: false,
    removalErrors: new Map<string, string>(),
    removals: [] as string[],
  };
  conn = NativeAgentConnection.create({ writeLine: line => {
    const request = JSON.parse(line);
    if (request.id == null) return;
    const reply = (result: unknown) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }) + "\n");
    const reject = (code: string) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code, message: code } }) + "\n");
    if (request.method === "session/queue/list") {
      if (state.failList) reject("InternalError"); else reply({ entries: state.entries });
      return;
    }
    if (request.method === "session/queue/update") {
      state.updates.push(request.params);
      reply({});
      return;
    }
    if (request.method === "session/queue/remove") {
      const id = request.params.queueItemId;
      state.removals.push(id);
      const error = state.removalErrors.get(id);
      if (!error || error === "QueueItemNotFound") state.entries = state.entries.filter(item => item.queueItemId !== id);
      if (error) reject(error); else reply({});
      return;
    }
    reply(request.method === "session/new" ? { session: { id: "root", title: "Root" } }
      : request.method === "subscription/create" ? { subscriptionId: "sub", snapshots: [], replay: [] }
      : request.method === "session/items/list" ? { data: [], nextCursor: null }
      : request.method === "turn/steer" || request.method === "session/queue/push"
        ? { outcome: "alreadyAccepted", queueItemId: "accepted", turnId: "prior-turn" } : {});
  } });
  await conn.newSession();
  return { conn, state };
}

test("clearQueue does not pretend durable messages disappeared when listing fails", async () => {
  const { conn, state } = await harness();
  try {
    const before = await conn.getQueue();
    state.failList = true;
    await assert.rejects(conn.clearQueue(), /InternalError/);
    assert.deepEqual(await conn.getQueue(), before);
    assert.deepEqual(state.removals, []);
  } finally { await conn.dispose(); }
});

test("clearQueue continues after already-consumed entries and confirms the remainder", async () => {
  const { conn, state } = await harness();
  try {
    state.removalErrors.set("first", "QueueItemNotFound");
    assert.deepEqual(await conn.clearQueue(), { steering: [], followUp: [], ordered: [] });
    assert.deepEqual(state.removals, ["first", "second"]);
  } finally { await conn.dispose(); }
});

test("partial remove failure surfaces the error and keeps remaining durable input visible", async () => {
  const { conn, state } = await harness();
  try {
    state.removalErrors.set("first", "InternalError");
    await assert.rejects(conn.clearQueue(), /InternalError/);
    assert.deepEqual(state.removals, ["first", "second"]);
    assert.deepEqual(await conn.getQueue(), {
      steering: [], followUp: ["first"],
      ordered: [{ id: "first", text: "first", delivery: "followUp", blocked: false }],
    });
  } finally { await conn.dispose(); }
});

test("alreadyAccepted receipts refresh the queue without adding another message", async () => {
  const { conn, state } = await harness();
  try {
    state.entries = [];
    await conn.followUp("already consumed");
    conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "turn/started", params: { turn: { id: "turn", sessionId: "root" } } }) + "\n");
    await conn.steer("already consumed");
    assert.deepEqual(await conn.getQueue(), { steering: [], followUp: [], ordered: [] });
  } finally { await conn.dispose(); }
});

test("recovery-held steer identifies itself and blocks later input", () => {
  assert.deepEqual(nativeNotificationToPiEvents("queue/updated", { queue: [
    { ...entry("held", "steer"), recoveryRequired: true },
    { ...entry("next", "steer"), position: 1 },
  ] }, createStreamState()), [{
    type: "session_action_update", actions: {
      queuedCount: 2, steering: ["held", "next"], followUps: [], ordered: [
        { id: "held", text: "held", delivery: "steer", blocked: false, recoveryRequired: true },
        { id: "next", text: "next", delivery: "steer", blocked: true },
      ],
    },
  }]);
});

test("editing long multiline input uses the complete payload, not the truncated preview", async () => {
  const { conn, state } = await harness();
  try {
    const full = "a".repeat(140) + "\nSecond line must not disappear.\nThird line.";
    state.entries = [{ ...entry("long"), preview: full.slice(0, 80), input: [{ type: "text", text: full }] }];
    await conn.followUp("refresh");
    const queue = await conn.getQueue();
    assert.equal(queue.followUp[0], full);
    assert.equal(queue.ordered?.[0].preview, full.slice(0, 80));
    assert.equal(await conn.mutateQueuedMessage("followUp", 0, full, { type: "replace", lane: "followUp", queueItemId: "long", text: full + "\nEdited" }), "applied");
    assert.deepEqual(state.updates, [{ sessionId: "root", queueItemId: "long", input: [{ type: "text", text: full + "\nEdited" }] }]);
  } finally { await conn.dispose(); }
});

test("image-and-text payload cannot be silently replaced by a text-only queue edit", async () => {
  const { conn, state } = await harness();
  try {
    state.entries = [{ ...entry("image"), preview: "Inspect attached image", input: [
      { type: "text", text: "Inspect attached image\nKeep the second line" },
      { type: "localImage", path: "/tmp/fixture.png" },
    ] }];
    const original = structuredClone(state.entries);
    await conn.followUp("refresh");
    const queue = await conn.getQueue();
    assert.equal(queue.ordered?.[0].editable, false);
    assert.equal(await conn.mutateQueuedMessage("followUp", 0, queue.followUp[0], { type: "replace", lane: "followUp", queueItemId: "image", text: "Changed" }), "rejected");
    assert.deepEqual(state.updates, []);
    assert.deepEqual(state.entries, original);
  } finally { await conn.dispose(); }
});

test("server revisions prevent late live events from resurrecting consumed input", async () => {
  const { conn } = await harness();
  try {
    const push = (revision: number, queue: unknown[]) => conn.pushChunk(JSON.stringify({ jsonrpc: "2.0", method: "queue/updated", params: { sessionId: "root", revision, queue } }) + "\n");
    push(8, []);
    push(7, [entry("stale")]);
    assert.deepEqual(await conn.getQueue(), { steering: [], followUp: [], ordered: [] });
    push(9, [entry("current")]);
    assert.deepEqual((await conn.getQueue()).followUp, ["current"]);
  } finally { await conn.dispose(); }
});

test("held queue rows render the partial-execution warning and explicit recovery route", async () => {
  const { InteractiveMode } = await import("../lib/coding-agent/src/modes/interactive/interactive-mode.js");
  const { Container } = await import("../lib/tui/src/tui.js");
  const { initTheme } = await import("@earendil-works/pi-coding-agent/theme");
  initTheme("dark", true);
  const mode = Object.create(InteractiveMode.prototype) as any;
  mode.connectionState = { sessionActions: {
    steering: ["held"], followUps: [], ordered: [
      { id: "held-id", text: "held", delivery: "steer", blocked: false, recoveryRequired: true },
    ],
  } };
  mode.pendingMessagesContainer = new Container();
  mode.pendingBashComponents = [];
  mode.queuedMessagesContainer = new Container();
  mode.getAppKeyDisplay = () => "Alt+Up";
  mode.isRecognizedSlashCommand = () => false;
  mode.updatePendingMessagesDisplay();
  const output = mode.queuedMessagesContainer.render(120).join("\n").replace(/\x1b\[[0-9;]*m/g, "");
  assert.match(output, /Held · review required/);
  assert.match(output, /may already have partially executed/);
  assert.match(output, /session\/queue\/retry/);
  assert.match(output, /no automatic retry/);
});
