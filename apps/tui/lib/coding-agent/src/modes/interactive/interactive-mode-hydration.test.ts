import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "./interactive-mode.js";
import { Container } from "@earendil-works/pi-tui";
import { ToolExecutionComponent } from "./components/tool-execution.js";
import { initTheme } from "./theme/theme.js";

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

test("command-triggered transcript rebuild fences live events until history is rendered", async () => {
  const { mode, rows, emit } = harness();
  const history = deferred(), loading = deferred();
  mode.subscribeToAgent();
  mode.resetCurrentSessionRenderState = () => { rows.length = 0; };
  mode.renderInitialMessages = async () => { loading.resolve(); await history.promise; rows.push("history"); };
  mode.updatePendingMessagesDisplay = () => {};
  mode.syncWorkingLoader = () => {};
  const render = mode.renderCurrentSessionState();
  await loading.promise;
  const live = emit("live");
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(rows, []);
  history.resolve();
  await Promise.all([render, live]);
  assert.deepEqual(rows, ["history", "live"]);
});

test("obsolete command rebuild cannot reset a newly selected session", async () => {
  const { mode, rows } = harness();
  const pending = deferred();
  mode.sessionEventQueue = pending.promise;
  mode.resetCurrentSessionRenderState = () => { rows.push("reset"); };
  mode.renderInitialMessages = async () => { rows.push("render"); };
  mode.updatePendingMessagesDisplay = () => {};
  mode.syncWorkingLoader = () => {};
  const render = mode.renderCurrentSessionState();
  mode.sessionEventGeneration++;
  pending.resolve();
  await render;
  assert.deepEqual(rows, []);
});

test("compaction rebuild fences queued live messages already covered by history", async () => {
  const { mode, rows, emit } = harness();
  const history = deferred();
  mode.agentConnection.getInitialSnapshot = async () => {
    await history.promise;
    return { messages: ["queued prompt"], completedItemRevisions: { queued: 2 } };
  };
  mode.renderResyncedSession = async (snapshot: any) => {
    mode.completedTranscriptItems = snapshot.completedItemRevisions;
    rows.splice(0, rows.length, ...snapshot.messages);
  };
  mode.subscribeToAgent();
  mode.handleEvent = async (event: any) => {
    if (event.text === "compact") await mode.rebuildChatFromMessages();
    else rows.push(event.text);
  };
  const compact = emit("compact");
  await new Promise(resolve => setImmediate(resolve));
  const replayed = emit("queued prompt", { id: "queued", revision: 2 });
  const distinct = emit("queued prompt", { id: "another", revision: 1 });
  history.resolve();
  await Promise.all([compact, replayed, distinct]);
  assert.deepEqual(rows, ["queued prompt", "queued prompt"]);
});

test("compaction history completing after session switch cannot replace target transcript", async () => {
  const { mode, rows } = harness();
  const history = deferred();
  mode.agentConnection.getInitialSnapshot = async () => { await history.promise; return {}; };
  mode.renderResyncedSession = async () => rows.push("obsolete history");
  const rebuild = mode.rebuildChatFromMessages();
  mode.sessionEventGeneration++;
  history.resolve();
  await rebuild;
  assert.deepEqual(rows, []);
});

test("compaction snapshot keeps one real tool row across queued turn lifecycle events", async () => {
  initTheme("dark");
  const mode = Object.create(InteractiveMode.prototype) as any;
  const history = deferred(), loading = deferred();
  let listener!: (event: any) => Promise<void>;
  Object.defineProperty(mode, "settingsManager", { value: {
    getShowImages: () => false, getShowTerminalProgress: () => false,
  } });
  Object.assign(mode, {
    sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0,
    completedTranscriptItems: {}, pendingTools: new Map(), pendingToolCreations: new Set(),
    startedToolCalls: new Set(), pendingToolGeneration: 0, ipythonToolComponents: new Map(),
    lateIpythonSentAgentMessages: new Map(), toolDefinitionCache: new Map(),
    chatContainer: new Container(), ui: { requestRender() {} }, footer: { invalidate() {} },
    isInitialized: true, workingVisible: false, activityTracker: { handleEvent() {} },
    agentConnection: {
      subscribe(next: typeof listener) { listener = next; return () => {}; },
      async getInitialSnapshot() {
        loading.resolve();
        await history.promise;
        return {
          state: { isBashRunning: false, isStreaming: true, messageCount: 1 },
          messages: [{ role: "assistant", content: [{
            type: "toolCall", id: "call-1", name: "read", arguments: { path: "file.txt" },
          }], stopReason: "toolUse", timestamp: 1 }],
          completedItemRevisions: { "tool-item": 2 },
          transcriptTurns: { compact: "terminal", followup: "started" }, children: [],
        };
      },
      getToolDefinition: async () => undefined,
    },
    showError(error: unknown) { throw new Error(String(error)); },
    isBashRunning: () => false,
    applyConnectionStateSnapshot(state: unknown) { mode.connectionState = state; },
    refreshQueueSelectionFromState() {}, restoreTurnStartFromMessages() {}, replaceSubagentSummary() {},
    updatePendingMessagesDisplay() {}, updateTerminalTitle() {}, setGoalAnnouncementBaseline() {},
    getGoalState() {}, syncGoalTray() {}, syncWorkingLoader() {}, updateEditorBorderColor() {},
    preloadToolDefinitions: async () => {}, updateWorkingPulse() {}, getCurrentCwd: () => process.cwd(),
    updateWorkingLoaderMessage() {}, renderRecap() {}, stopWorkingLoader() {},
    refreshConnectionContextUsage: async () => {}, flushPendingBashComponents() {},
    checkShutdownRequested: async () => {},
    // Omit assistant chrome only. History replay, live handling, tool creation,
    // pending-tool ownership, and ToolExecutionComponent itself remain real.
    addMessageToChat() {},
  });
  mode.subscribeToAgent();
  const emit = (event: any, sourceTurn?: { id: string; phase: "started" | "terminal" }) =>
    listener({ type: "session_event", event, sourceTurn });
  const compaction = emit({ type: "compaction_end", reason: "manual", result: {}, aborted: false });
  await loading.promise;
  const queued = [
    emit({ type: "agent_start" }, { id: "followup", phase: "started" }),
    emit({ type: "agent_end" }, { id: "compact", phase: "terminal" }),
    emit({ type: "tool_execution_start", toolCallId: "call-1", toolName: "read", args: { path: "file.txt" } }),
  ];
  history.resolve();
  await Promise.all([compaction, ...queued]);

  const toolRows = () => mode.chatContainer.children.filter((child: unknown) => child instanceof ToolExecutionComponent);
  assert.equal(toolRows().length, 1, "queued lifecycle events must not discard the snapshot's pending tool ownership");
  const component = toolRows()[0];
  assert.equal(mode.pendingTools.get("call-1"), component);
  assert.equal(mode.connectionState.isStreaming, true, "old compaction completion must not terminate the hydrated follow-up");

  await emit({ type: "tool_execution_end", toolCallId: "call-1", toolName: "read",
    result: { content: [{ type: "text", text: "file contents" }] }, isError: false });
  assert.deepEqual(toolRows(), [component]);
  assert.equal(mode.pendingTools.size, 0);
  assert.match(component.render(100).join("\n"), /file contents/);
  await emit({ type: "agent_end" }, { id: "followup", phase: "terminal" });
  assert.equal(mode.connectionState.isStreaming, false, "the actual follow-up terminal event must still apply");
  await emit({ type: "agent_start" }, { id: "next-turn", phase: "started" });
  assert.equal(mode.connectionState.isStreaming, true, "future turn starts must not be suppressed");
});
