import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "./interactive-mode.js";

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

function harness() {
  const mode = Object.create(InteractiveMode.prototype) as any;
  const rows: string[] = [];
  let listener: (event: any) => Promise<void>;
  Object.assign(mode, {
    sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0,
    completedTranscriptItems: {},
    agentConnection: { subscribe: (next: typeof listener) => { listener = next; return () => {}; } },
    handleEvent: async (event: any) => { rows.push(event.text); },
    showError: (error: unknown) => { throw new Error(String(error)); },
  });
  return { mode, rows, emit: (text: string, sourceItem?: { id: string; revision?: number }) =>
    listener({ type: "session_event", event: { type: "message_start", text }, sourceItem }) };
}

test("initial history and tool preload finish before live events, even events at subscribe", async () => {
  const { mode, rows, emit } = harness();
  const history = deferred(), preload = deferred(), subscribed = deferred();
  let live!: Promise<void>;
  mode.rebindCurrentSession = async () => {
    mode.subscribeToAgent();
    live = emit("live");
    subscribed.resolve();
  };
  mode.renderInitialMessages = async () => { await history.promise; await preload.promise; rows.push("history"); };
  const init = mode.initializeSessionTranscript();
  await subscribed.promise;
  assert.deepEqual(rows, []);
  history.resolve();
  await Promise.resolve();
  assert.deepEqual(rows, []);
  preload.resolve();
  await Promise.all([init, live]);
  assert.deepEqual(rows, ["history", "live"]);
});

test("snapshot-covered terminal item events deduplicate by identity/revision, not text", async () => {
  const { mode, rows, emit } = harness();
  const history = deferred();
  mode.rebindCurrentSession = async () => { mode.subscribeToAgent(); };
  mode.renderInitialMessages = async () => {
    await history.promise;
    mode.completedTranscriptItems = { old: 3 };
    rows.push("same text");
  };
  const init = mode.initializeSessionTranscript();
  await new Promise(resolve => setImmediate(resolve));
  const events = [emit("stale start", { id: "old", revision: 1 }),
    emit("stale delta", { id: "old" }), emit("stale end", { id: "old", revision: 3 }),
    emit("same text", { id: "new", revision: 1 }), emit("new revision", { id: "old", revision: 4 })];
  history.resolve();
  await Promise.all([init, ...events]);
  assert.deepEqual(rows, ["same text", "same text", "new revision"]);
});

test("switching generation while history loads drops queued source events", async () => {
  const { mode, rows, emit } = harness();
  const history = deferred();
  mode.rebindCurrentSession = async () => { mode.subscribeToAgent(); await history.promise; };
  mode.renderInitialMessages = async () => { rows.push("obsolete history"); };
  const init = mode.initializeSessionTranscript();
  await new Promise(resolve => setImmediate(resolve));
  const live = emit("obsolete live");
  mode.sessionEventGeneration++;
  history.resolve();
  await Promise.all([init, live]);
  assert.deepEqual(rows, []);
});

test("failed initial load releases the event barrier rather than deadlocking", async () => {
  const { mode, rows, emit } = harness();
  mode.rebindCurrentSession = async () => { mode.subscribeToAgent(); };
  mode.renderInitialMessages = async () => { throw new Error("load failed"); };
  await assert.rejects(mode.initializeSessionTranscript(), /load failed/);
  await emit("live after failure");
  assert.deepEqual(rows, ["live after failure"]);
});

import { hydrateQueuedTextEvent } from "./transcript-hydration.js";
test("queued pre-snapshot deltas receive the birth prefix exactly once without changing the input event", () => {
  const message = { role: "assistant", content: [{ type: "text", text: "suffix" }] };
  const event = { type: "session_event", sourceItem: { id: "a", missingTextBaseIds: ["a"] }, event: { type: "message_update", message } } as any;
  const prefixes = { a: { kind: "text", prefix: "prefix " } } as const;
  assert.deepEqual((hydrateQueuedTextEvent(event, prefixes) as any).message.content, [{ type: "text", text: "prefix suffix" }]);
  assert.deepEqual(message.content, [{ type: "text", text: "suffix" }]);
  event.sourceItem.missingTextBaseIds = [];
  assert.equal(hydrateQueuedTextEvent(event, prefixes), event.event);
});
