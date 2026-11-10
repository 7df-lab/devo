import assert from "node:assert/strict";
import { test } from "node:test";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import {
  createAgentSessionMessage,
  startsAgentRun,
} from "../lib/coding-agent/src/core/agent-messages.js";
import type { CustomMessage } from "../lib/coding-agent/src/core/messages.js";
import type { AgentConnectionSessionEvent } from "../lib/coding-agent/src/modes/agent-connection/types.js";
import { AgentActivityTracker } from "../lib/coding-agent/src/modes/interactive/agent-activity.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import type { HostMessageDelivery, HostMessageSource } from "./generated/protocol/native.js";
import { createStreamState, nativeNotificationToPiEvents } from "./native-to-pi/events.js";
import { nativeItemToAgentMessages } from "./native-to-pi/messages.js";

function hostEnvelope(source: HostMessageSource, delivery: HostMessageDelivery, timestamp = 2000) {
  return {
    id: "host-item",
    createdAt: timestamp,
    item: {
      type: "userMessage",
      origin: { source, delivery, eventId: "host-event", sender: "worker-1" },
      content: [{ type: "text", text: "Result ready" }],
    },
  };
}

function assistantMessage(timestamp: number, stopReason: AssistantMessage["stopReason"] = "stop"): AssistantMessage {
  return {
    role: "assistant",
    content: [{ type: "text", text: "Previous response" }],
    api: "openai-responses",
    provider: "openai",
    model: "test-model",
    usage: {
      input: 0, output: 64, cacheRead: 0, cacheWrite: 0, totalTokens: 64,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason,
    timestamp,
  };
}

interface ActivityHarness {
  activityTracker: AgentActivityTracker;
  contextUsageTokenBaseline: number;
  agentRunFileChanges: Map<string, string>;
  turnStartedAt: number | undefined;
  workingStartedAt: number | undefined;
  displayedMessages: AgentMessage[];
  shortcutClears: number;
  recapRenders: number;
  handleEvent(event: AgentConnectionSessionEvent): Promise<void>;
  restoreTurnStartFromMessages(messages: readonly AgentMessage[]): void;
}

function activityHarness(): ActivityHarness {
  const tracker = new AgentActivityTracker();
  tracker.handleEvent({ type: "message_end", message: assistantMessage(1000) });
  tracker.handleEvent({ type: "tool_execution_start", toolCallId: "tool-1", toolName: "test", args: {} });
  const mode = Object.create(InteractiveMode.prototype) as ActivityHarness;
  Object.assign(mode, {
    isInitialized: true,
    activityTracker: tracker,
    contextUsageTokenBaseline: 64,
    agentRunFileChanges: new Map([["previous.ts", "previous run"]]),
    turnStartedAt: undefined,
    workingStartedAt: 1500,
    displayedMessages: [],
    shortcutClears: 0,
    recapRenders: 0,
    footer: { invalidate() {} },
    ui: { requestRender() {} },
    updateConnectionStateFromEvent() {},
    updateWorkingLoaderMessage() {},
    isAgentStreaming: () => true,
    clearShortcutGuide: () => { mode.shortcutClears++; },
    renderRecap: () => { mode.recapRenders++; },
    addMessageToChat: (message: AgentMessage) => { mode.displayedMessages.push(message); },
  });
  return mode;
}

for (const source of ["agent", "backgroundTask", "schedule"] as const) {
  for (const delivery of ["followUp", "steer"] as const) {
    test(`${source}/${delivery} live delivery only resets activity at a follow-up boundary`, async () => {
      const mode = activityHarness();
      const envelope = hostEnvelope(source, delivery);
      const [message] = nativeItemToAgentMessages(envelope) as unknown as AgentMessage[];
      assert.ok(message);
      assert.equal(startsAgentRun(message), delivery === "followUp");

      const events = nativeNotificationToPiEvents("item/started", { item: envelope }, createStreamState());
      for (const event of events) {
        await mode.handleEvent(event as unknown as AgentConnectionSessionEvent);
      }

      const followUp = delivery === "followUp";
      assert.deepEqual({
        status: mode.activityTracker.getStatus(),
        baseline: mode.contextUsageTokenBaseline,
        fileChanges: [...mode.agentRunFileChanges],
        turnStartedAt: mode.turnStartedAt,
        workingStartedAt: mode.workingStartedAt,
        shortcutClears: mode.shortcutClears,
        recapRenders: mode.recapRenders,
        displayedMessages: mode.displayedMessages,
      }, {
        status: { activity: followUp ? "waiting" : "executing", direction: "up", tokens: followUp ? 0 : 64 },
        baseline: followUp ? 0 : 64,
        fileChanges: followUp ? [] : [["previous.ts", "previous run"]],
        turnStartedAt: followUp ? 2000 : undefined,
        workingStartedAt: followUp ? 2000 : 1500,
        shortcutClears: followUp ? 1 : 0,
        recapRenders: followUp ? 1 : 0,
        displayedMessages: [message],
      });

      mode.activityTracker.handleEvent({ type: "message_end", message: assistantMessage(3000) });
      assert.deepEqual(mode.activityTracker.getStatus(), {
        activity: "waiting", direction: "up", tokens: followUp ? 64 : 128,
      });
    });
  }

  test(`${source} replay restores the follow-up start, ignoring a later host steer`, () => {
    const mode = activityHarness();
    const followUp = nativeItemToAgentMessages(hostEnvelope(source, "followUp", 2000)) as unknown as AgentMessage[];
    const steer = nativeItemToAgentMessages(hostEnvelope(source, "steer", 4000)) as unknown as AgentMessage[];
    mode.restoreTurnStartFromMessages([
      { role: "user", content: "Previous prompt", timestamp: 500 },
      assistantMessage(1000),
      ...followUp,
      assistantMessage(3000, "toolUse"),
      ...steer,
    ]);
    assert.deepEqual({ turn: mode.turnStartedAt, working: mode.workingStartedAt }, { turn: 2000, working: 2000 });

    mode.restoreTurnStartFromMessages([assistantMessage(1000), ...steer]);
    assert.equal(mode.turnStartedAt, undefined, "a steer is not a new run's start");
  });
}

test("display labels and message text cannot create a host turn boundary", () => {
  for (const details of [undefined, null, "followUp", {}, { origin: null }, { origin: "followUp" }, { origin: {} }]) {
    const message: CustomMessage = {
      role: "custom", customType: "Agent message", content: "[host followUp] Result ready",
      details, display: true, timestamp: 2000,
    };
    const mode = activityHarness();
    mode.activityTracker.handleEvent({ type: "message_start", message });
    assert.deepEqual({ startsRun: startsAgentRun(message), status: mode.activityTracker.getStatus() }, {
      startsRun: false, status: { activity: "executing", direction: "up", tokens: 64 },
    });
  }
});

test("human and legacy agent inputs still reset activity and start runs", () => {
  const messages: AgentMessage[] = [
    { role: "user", content: "[Agent message] still human", timestamp: 2000 },
    createAgentSessionMessage({
      id: "agentmsg_1", source: "agent_message", message: "Legacy agent input",
      target: { activeSessionId: "active-1", sessionId: "session-1" },
    }, 2000),
  ];
  for (const message of messages) {
    const mode = activityHarness();
    mode.activityTracker.handleEvent({ type: "message_start", message });
    assert.deepEqual({ startsRun: startsAgentRun(message), status: mode.activityTracker.getStatus() }, {
      startsRun: true, status: { activity: "waiting", direction: "up", tokens: 0 },
    });
  }
});
