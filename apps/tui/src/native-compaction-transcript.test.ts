import assert from "node:assert/strict";
import { test } from "node:test";
import { nativeItemsToAgentMessages } from "./native-to-pi/messages.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { CompactionSummaryMessageComponent } from "../lib/coding-agent/src/modes/interactive/components/compaction-summary-message.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { NativeAgentConnection } from "./native-agent-connection.js";

const item = (id: string, summary = "Summary of retained work") => ({
  id, createdAt: "2026-11-11T00:00:00Z", state: "completed",
  item: { type: "contextCompaction", trigger: "manual", summary,
    before: { totalTokens: 12000, measured: true }, after: { totalTokens: 3000, measured: true } },
});
const user = { id: "u", createdAt: "2026-11-11T00:00:00Z", item: { type: "userMessage", content: [{ type: "text", text: "Continue" }] } };

test("Native compaction survives projection and preserves repeated summaries in journal order", () => {
  const messages = nativeItemsToAgentMessages([user, item("a"), user, item("b", "Second summary"), user]);
  assert.deepEqual(messages.map(m => m.role), ["user", "compactionSummary", "user", "compactionSummary", "user"]);
  assert.deepEqual(messages[1], { role: "compactionSummary", summary: "Summary of retained work", tokensBefore: 12000,
    tokensBeforeMeasured: true, tokensAfter: 3000, transcriptOrder: "native", timestamp: Date.parse(user.createdAt) });
  const mode = Object.create(InteractiveMode.prototype) as any;
  assert.deepEqual(mode.orderMessagesForTranscript(messages), messages);
  // Active-path API supplies the branch selected by rewind, not abandoned items.
  assert.deepEqual(mode.orderMessagesForTranscript(nativeItemsToAgentMessages([user])), nativeItemsToAgentMessages([user]));
});

test("missing occupancy is unknown, wrappers are hidden, incomplete items are not successes", () => {
  const source = item("c", "<compaction_summary>\nActual summary\n</compaction_summary>");
  source.item.before = { totalTokens: 0, measured: false };
  source.item.after = { totalTokens: 0, measured: false };
  const [message] = nativeItemsToAgentMessages([source]);
  assert.equal(message.summary, "Actual summary");
  assert.equal(message.tokensBeforeMeasured, false);
  assert.equal(message.tokensAfter, undefined);
  for (const state of ["running", "waiting", "failed", "interrupted", "lost"]) {
    assert.deepEqual(nativeItemsToAgentMessages([{ ...source, state }]), []);
  }
});

test("live completion and recreated/switching connections load the same paginated history", async () => {
  const pages = [[item("a"), user], [item("b")]];
  const make = () => {
    const connection = Object.create(NativeAgentConnection.prototype) as any;
    Object.assign(connection, { sessionId: "one", sessionHistoryRevision: 0, messages: [], historySnapshotItems: new WeakMap(), historySnapshotTurns: new WeakMap(), rpc: {
      request: async (method: string, params: any) => {
        if (method === "session/turns/list") return { data: [] };
        if (params.sessionId === "two") return { data: [user] };
        return params.cursor ? { data: pages[1] } : { data: pages[0], nextCursor: "next" };
      },
    } });
    return connection;
  };
  const connection = make();
  const completed = nativeNotificationToPiEvents("item/completed", item("b"), createStreamState());
  assert.equal(completed[0]?.type, "compaction_end");
  assert.ok(completed[0]?.result);
  const live = await connection.getMessages();
  connection.sessionId = "two";
  assert.deepEqual((await connection.getMessages()).map((m: any) => m.role), ["user"]);
  connection.sessionId = "one";
  assert.deepEqual(await connection.getMessages(), live);
  assert.deepEqual(await make().getMessages(), live);
});

test("compaction card has an independent padded block and expands full summary", () => {
  initTheme("dark", false);
  const summary = "Detailed evidence ".repeat(50);
  const message = nativeItemsToAgentMessages([item("card", summary)])[0] as any;
  const card = new CompactionSummaryMessageComponent(message);
  const plain = (lines: string[]) => lines.map(line => line.replace(/\x1b\[[0-9;]*m/g, ""));
  const collapsed = plain(card.render(80));
  assert.equal(collapsed[0].trim(), "");
  assert.equal(collapsed.at(-1)?.trim(), "");
  assert.match(collapsed.join("\n"), /Compaction · completed/);
  assert.match(collapsed.join("\n"), /12,000 → 3,000 tokens/);
  card.setExpanded(true);
  const expanded = plain(card.render(80));
  assert.ok(expanded.length > collapsed.length);
  assert.match(expanded.join("\n"), /Compaction summary/);
});

test("successful summaries mentioning skipped work still trigger transcript refresh", () => {
  const events = nativeNotificationToPiEvents("item/completed", item("ok", "Skip old steps; the earlier reply was too short."), createStreamState());
  assert.equal(events[0]?.type, "compaction_end");
  assert.ok(events[0]?.result);
  assert.equal(events[0]?.errorMessage, undefined);
});

test("failed Native compaction summary still carries the too-short warning", () => {
  const envelope = item("short", "Compaction failed: Session is too short to compact");
  const [event] = nativeNotificationToPiEvents("item/completed", envelope, createStreamState());
  assert.equal(event.result, undefined);
  assert.equal(event.errorSeverity, "warning");
  assert.match(String(event.errorMessage), /too short/);
});


test("completed-lifecycle failure is never projected as a successful summary", () => {
  assert.deepEqual(nativeItemsToAgentMessages([item("failed", "Compaction failed: provider disconnected")]), []);
  const [event] = nativeNotificationToPiEvents("item/completed", item("failed", "Compaction failed: provider disconnected"), createStreamState());
  assert.equal(event.result, undefined);
  assert.equal(event.errorSeverity, "error");
});


test("legacy pi compaction retains retained-count and timestamp ordering", () => {
  const mode = Object.create(InteractiveMode.prototype) as any;
  const older = { role: "user", timestamp: 1 };
  const later = { role: "user", timestamp: 3 };
  for (const summary of [
    { role: "compactionSummary", timestamp: 2, retainedMessageCount: 1 },
    { role: "compactionSummary", timestamp: 2 },
  ]) assert.deepEqual(mode.orderMessagesForTranscript([summary, older, later]), [older, summary, later]);
});

test("InteractiveMode live completion rebuilds through history and clears previous transcript", async () => {
  const messages = nativeItemsToAgentMessages([user, item("live")]);
  const mode = Object.create(InteractiveMode.prototype) as any;
  const operations: string[] = [];
  Object.assign(mode, {
    isInitialized: true, footer: { invalidate() {} }, updateConnectionStateFromEvent() {},
    activityTracker: { handleEvent() {} }, updateWorkingLoaderMessage() {}, syncWorkingLoader() {},
    agentConnection: { getInitialSnapshot: async () => ({ messages }) },
    isCurrentConnectionGeneration: () => true,
    renderResyncedSession: async (context: any) => {
      assert.deepEqual(context.messages, messages); operations.push("replace");
    },
    refreshConnectionContextUsage: async () => operations.push("usage"),
    ui: { requestRender: () => operations.push("render") },
  });
  Object.defineProperty(mode, "settingsManager", { value: { getShowTerminalProgress: () => false } });
  const [event] = nativeNotificationToPiEvents("item/completed", item("live"), createStreamState());
  await mode.handleEvent(event);
  assert.deepEqual(operations, ["replace", "usage", "render"]);
});
