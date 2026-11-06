/**
 * Trace: L2-DES-APP-010
 * Verifies: quota failures survive Native projection, rendering, and restore.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { AssistantMessageComponent } from "../../lib/coding-agent/src/modes/interactive/components/assistant-message.js";
import { initTheme } from "../../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { NativeAgentConnection } from "../native-agent-connection.js";
import { createStreamState, nativeNotificationToPiEvents } from "./events.js";
import { applyFailedTurnsToAgentMessages, nativeItemsToAgentMessages } from "./messages.js";

initTheme("dark");

const quotaMessage = "You have no credits remaining. Please add credits to continue.";
const partialText = "The answer so far.";
const completedAt = "2026-10-24T00:00:00.000Z";
const failedTurn = {
  id: "turn-quota",
  status: "failed",
  error: { errorCode: "QUOTA_EXCEEDED_ERROR", message: quotaMessage },
  completedAt,
};
const usage = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 };

function renderedLines(messages: Array<Record<string, unknown>>): string[] {
  return messages.flatMap((message) =>
    new AssistantMessageComponent(message as unknown as AssistantMessage)
      .render(160)
      .map((line) => stripVTControlCharacters(line).trim())
      .filter(Boolean),
  );
}

for (const textState of ["no tokens", "streaming text", "completed text"] as const) {
  for (const statusNotification of ["absent", "before completion", "after completion"] as const) {
    test(`quota failure: ${textState}, failed status ${statusNotification}`, () => {
      const state = createStreamState();
      assert.deepEqual(nativeNotificationToPiEvents("turn/started", { turn: { id: failedTurn.id } }, state), [
        { type: "agent_start" },
      ]);
      const endedMessages: Array<Record<string, unknown>> = [];
      if (textState !== "no tokens") {
        nativeNotificationToPiEvents("item/assistantMessage/delta", { itemId: "a1", delta: partialText }, state);
        if (textState === "completed text") {
          const itemEvents = nativeNotificationToPiEvents("item/completed", {
            id: "a1", item: { type: "assistantMessage", text: partialText },
          }, state);
          endedMessages.push(...itemEvents.filter((event) => event.type === "message_end")
            .map((event) => event.message as Record<string, unknown>));
        }
      }
      if (statusNotification === "before completion") {
        assert.deepEqual(nativeNotificationToPiEvents("turn/statusChanged", {
          turnId: failedTurn.id, status: "failed",
        }, state), [{ type: "agent_end" }]);
      }

      const events = nativeNotificationToPiEvents("turn/completed", { turn: failedTurn }, state);
      const message = events[0]?.message as Record<string, unknown>;
      assert.equal(typeof message?.timestamp, "number");
      assert.deepEqual(events, [
        {
          type: "message_end",
          message: {
            role: "assistant",
            content: [{ type: "text", text: textState === "streaming text" ? partialText : "" }],
            timestamp: message.timestamp,
            stopReason: "error",
            errorMessage: quotaMessage,
            usage,
          },
        },
        { type: "agent_end" },
      ]);
      endedMessages.push(message);
      assert.deepEqual(renderedLines(endedMessages), [
        ...(textState === "no tokens" ? [] : [partialText]),
        `Error: ${quotaMessage}`,
      ]);
      assert.deepEqual(state, {
        ...createStreamState(),
        assistantTextByItem: new Map(textState === "streaming text" ? [["a1", partialText]] : []),
      });
      if (statusNotification === "after completion") {
        assert.deepEqual(nativeNotificationToPiEvents("turn/statusChanged", {
          turnId: failedTurn.id, status: "failed",
        }, state), [{ type: "agent_end" }]);
      }
      // The next turn must get a new stream, not reuse the failed one.
      assert.deepEqual(nativeNotificationToPiEvents("turn/started", { turn: { id: "next-turn" } }, state), [
        { type: "agent_start" },
      ]);
      const nextEvents = nativeNotificationToPiEvents("item/assistantMessage/delta", {
        itemId: "a2", delta: "Next answer.",
      }, state);
      assert.deepEqual(nextEvents.map((event) => event.type), ["message_start", "message_update"]);
      assert.deepEqual(renderedLines([nextEvents[1].message as Record<string, unknown>]), ["Next answer."]);
    });
  }
}

for (const text of ["", partialText]) {
  test(`quota failure ends connection activity with ${text ? "existing text" : "zero text"}`, async () => {
    const connection = NativeAgentConnection.create({ writeLine: () => assert.fail("unexpected RPC") });
    const events: Array<Record<string, unknown>> = [];
    const unsubscribe = connection.subscribe((event) => {
      if (event.type === "session_event") events.push(event.event as unknown as Record<string, unknown>);
    });
    try {
      const initialState = await connection.getState();
      connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", method: "turn/started", params: {
        turn: { id: failedTurn.id },
      } })}\n`);
      assert.deepEqual(await connection.getState(), { ...initialState, isStreaming: true });
      if (text) {
        connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", method: "item/assistantMessage/delta",
          params: { itemId: "a1", delta: text },
        })}\n`);
      }
      connection.pushChunk(`${JSON.stringify({ jsonrpc: "2.0", method: "turn/completed",
        params: { turn: failedTurn },
      })}\n`);
      assert.deepEqual(await connection.getState(), initialState);
      const terminalEvents = events.filter((event) => event.type === "message_end" || event.type === "agent_end");
      assert.deepEqual(terminalEvents.map((event) => event.type), ["message_end", "agent_end"]);
      assert.deepEqual(renderedLines([terminalEvents[0].message as Record<string, unknown>]), [
        ...(text ? [text] : []), `Error: ${quotaMessage}`,
      ]);
    } finally {
      unsubscribe();
      connection.dispose();
    }
  });

  for (const durableWarning of ["present", "absent"] as const) {
    test(`quota failure restore: ${text ? "existing text" : "zero text"}, warning ${durableWarning}`, () => {
      const timestamp = Date.parse(completedAt);
      const items = [
        ...(text ? [{ id: "a1", createdAt: completedAt, item: { type: "assistantMessage", text } }] : []),
        ...(durableWarning === "present" ? [{
          id: "quota-warning", createdAt: completedAt,
          item: { type: "warning", code: failedTurn.error.errorCode, message: quotaMessage },
        }] : []),
      ];
      const restored = applyFailedTurnsToAgentMessages(nativeItemsToAgentMessages(items), [failedTurn]);
      const expected: Array<Record<string, unknown>> = durableWarning === "present" && text ? [{
        role: "assistant", content: [{ type: "text", text }], timestamp,
        stopReason: null, errorMessage: null, usage,
      }] : [];
      expected.push({
        role: "assistant", content: [{ type: "text", text: durableWarning === "present" ? "" : text }],
        timestamp, stopReason: "error", errorMessage: quotaMessage, usage,
      });
      assert.deepEqual(restored, expected);
      assert.deepEqual(renderedLines(restored), [...(text ? [text] : []), `Error: ${quotaMessage}`]);
      assert.deepEqual(applyFailedTurnsToAgentMessages(restored, [failedTurn]), restored);
    });
  }
}
