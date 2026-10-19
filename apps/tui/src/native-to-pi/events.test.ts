/**
 * Trace: L2-DES-APP-010
 * Verifies: Native stream notifications project to InteractiveMode session_event shapes.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  abortUiSessionEvents,
  createStreamState,
  nativeNotificationToPiEvents,
} from "./events.js";
import { nativeItemsToAgentMessages } from "./messages.js";
import { projectToolStatusUpdated } from "./tools.js";

test("turn start/complete emit agent_start/agent_end", () => {
  const state = createStreamState();
  assert.deepEqual(nativeNotificationToPiEvents("turn/started", {}, state), [
    { type: "agent_start" },
  ]);
  assert.deepEqual(nativeNotificationToPiEvents("turn/completed", {}, state), [
    { type: "agent_end" },
  ]);
});

test("interrupted turn projects aborted message_end then agent_end", () => {
  const state = createStreamState();
  // Open a live assistant stream first.
  nativeNotificationToPiEvents(
    "item/assistantMessage/delta",
    { itemId: "a1", delta: "partial" },
    state,
  );
  const events = nativeNotificationToPiEvents(
    "turn/completed",
    { turn: { status: "interrupted" } },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  const msg = events[0]?.message as { stopReason?: string };
  assert.equal(msg.stopReason, "aborted");
  assert.equal(events.at(-1)?.type, "agent_end");
});

test("interrupted turn with pending tool closes tool then aborts", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/started",
    {
      id: "i1",
      item: {
        type: "toolCall",
        callId: "c-pending",
        toolName: "ipython",
        arguments: { code: "1" },
      },
    },
    state,
  );
  projectToolStatusUpdated({ callId: "c-pending", status: "in_progress" }, state.tools);

  const events = nativeNotificationToPiEvents(
    "turn/completed",
    { turn: { status: "interrupted" } },
    state,
  );
  assert.ok(events.some((e) => e.type === "tool_execution_end"));
  const end = events.find((e) => e.type === "tool_execution_end") as {
    result?: { details?: { status?: string } };
    isError?: boolean;
  };
  assert.equal(end?.result?.details?.status, "aborted");
  assert.equal(end?.isError, true);
  assert.equal(events.at(-1)?.type, "agent_end");
});

test("abortUiSessionEvents closes pending tools", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/started",
    {
      id: "i1",
      item: {
        type: "toolCall",
        callId: "c-abort",
        toolName: "ipython",
        arguments: { code: "1" },
      },
    },
    state,
  );
  projectToolStatusUpdated({ callId: "c-abort", status: "in_progress" }, state.tools);
  const events = abortUiSessionEvents(state);
  assert.ok(events.some((e) => e.type === "tool_execution_end"));
  assert.equal(events.at(-1)?.type, "agent_end");
});

test("interrupted turn with no stream still emits aborted message_end", () => {
  const state = createStreamState();
  const events = nativeNotificationToPiEvents(
    "turn/completed",
    { turn: { status: "interrupted" } },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  assert.equal((events[0]?.message as { stopReason?: string }).stopReason, "aborted");
  assert.equal(events[1]?.type, "agent_end");
});

test("failed turn with no stream still emits error message_end", () => {
  const state = createStreamState();
  const events = nativeNotificationToPiEvents(
    "turn/completed",
    {
      turn: {
        status: "failed",
        error: { message: "provider timeout after retries" },
      },
    },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  const msg = events[0]?.message as { stopReason?: string; errorMessage?: string };
  assert.equal(msg.stopReason, "error");
  assert.equal(msg.errorMessage, "provider timeout after retries");
  assert.equal(events[1]?.type, "agent_end");
});

test("failed turn with open stream closes with provider errorMessage", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/assistantMessage/delta",
    { itemId: "a1", delta: "partial" },
    state,
  );
  const events = nativeNotificationToPiEvents(
    "turn/completed",
    {
      turn: {
        status: "failed",
        error: { code: "PROVIDER_SERVER_ERROR", message: "500 boom" },
      },
    },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  const msg = events[0]?.message as { stopReason?: string; errorMessage?: string };
  assert.equal(msg.stopReason, "error");
  assert.equal(msg.errorMessage, "500 boom");
  assert.equal(events.at(-1)?.type, "agent_end");
});

test("turn/statusChanged failed defers Error chrome to turn/completed", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/assistantMessage/delta",
    { itemId: "a1", delta: "partial" },
    state,
  );
  assert.deepEqual(
    nativeNotificationToPiEvents("turn/statusChanged", { status: "failed", turnId: "t1" }, state),
    [{ type: "agent_end" }],
  );
  // Stream still open — turn/completed owns the error close.
  const completed = nativeNotificationToPiEvents(
    "turn/completed",
    { turn: { status: "failed", error: { message: "kept" } } },
    state,
  );
  assert.equal(completed[0]?.type, "message_end");
  assert.equal((completed[0]?.message as { errorMessage?: string }).errorMessage, "kept");
});

test("model/queryFailed projects auto_retry_end failure", () => {
  const state = createStreamState();
  assert.deepEqual(
    nativeNotificationToPiEvents(
      "model/queryFailed",
      { attempt: 5, error: { message: "exhausted" } },
      state,
    ),
    [
      {
        type: "auto_retry_end",
        success: false,
        attempt: 5,
        finalError: "exhausted",
      },
    ],
  );
});

test("assistant delta expands to message_start + message_update", () => {
  const state = createStreamState();
  const events = nativeNotificationToPiEvents(
    "item/assistantMessage/delta",
    { itemId: "a1", delta: "Hello" },
    state,
  );
  assert.equal(events[0]?.type, "message_start");
  assert.equal(events[1]?.type, "message_update");
  const msg = events[1]?.message as { content?: Array<{ text?: string }> };
  const text = msg?.content?.find((c) => c.text != null)?.text;
  assert.equal(text, "Hello");
});

test("assistant item completed without deltas falls back to item text", () => {
  const state = createStreamState();
  // Delta-less client (proxy transport, mid-turn attach): only item
  // lifecycle arrives. The started item carries no text yet.
  const started = nativeNotificationToPiEvents(
    "item/started",
    { id: "a1", item: { type: "assistantMessage", text: "" } },
    state,
  );
  assert.equal(started[0]?.type, "message_start");
  const events = nativeNotificationToPiEvents(
    "item/completed",
    { id: "a1", item: { type: "assistantMessage", text: "banana" } },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  const msg = events[0]?.message as { content?: Array<{ text?: string }> };
  const text = msg?.content?.find((c) => c.text != null)?.text;
  assert.equal(text, "banana");
});

test("assistant item completed prefers accumulated delta text over item text", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/started",
    { id: "a1", item: { type: "assistantMessage", text: "" } },
    state,
  );
  nativeNotificationToPiEvents(
    "item/assistantMessage/delta",
    { itemId: "a1", delta: "streamed" },
    state,
  );
  const events = nativeNotificationToPiEvents(
    "item/completed",
    { id: "a1", item: { type: "assistantMessage", text: "streamed" } },
    state,
  );
  assert.equal(events[0]?.type, "message_end");
  const msg = events[0]?.message as { content?: Array<{ text?: string }> };
  const text = msg?.content?.find((c) => c.text != null)?.text;
  assert.equal(text, "streamed");
});

test("tool call start projects tool_pending (not running yet)", () => {
  const state = createStreamState();
  const startedFlat = nativeNotificationToPiEvents(
    "item/started",
    {
      id: "i1",
      item: {
        type: "toolCall",
        callId: "c1",
        toolName: "ipython",
        arguments: { code: "1+1" },
      },
    },
    state,
  );
  const ev = startedFlat.find((e) => e.type === "tool_pending");
  assert.ok(ev);
  assert.equal(ev?.toolCallId, "c1");
  assert.equal(ev?.toolName, "ipython");
  assert.deepEqual(ev?.args, { code: "1+1" });
  assert.equal(startedFlat.some((e) => e.type === "tool_execution_start"), false);

  const ended = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "i2",
      item: {
        type: "toolResult",
        callId: "c1",
        output: { content: [{ type: "text", text: "2" }] },
        isError: false,
      },
    },
    state,
  );
  assert.ok(ended.some((e) => e.type === "tool_execution_start"));
  assert.equal(ended.at(-1)?.type, "tool_execution_end");
  assert.equal(ended.at(-1)?.toolCallId, "c1");
});

test("toolResult without content array is normalized for ToolExecutionComponent", () => {
  const state = createStreamState();
  const ended = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "i2",
      item: {
        type: "toolResult",
        callId: "c1",
        output: { status: "complete" },
      },
    },
    state,
  );
  const end = ended.find((e) => e.type === "tool_execution_end");
  assert.ok(end);
  const result = end?.result as { content?: Array<{ type?: string; text?: string }> };
  assert.ok(Array.isArray(result.content));
  assert.equal(result.content?.[0]?.type, "text");
});

test("ipython Mixed output nests InteractiveMode-shaped details for cell display", () => {
  const state = createStreamState();
  const ended = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "i2",
      item: {
        type: "toolResult",
        callId: "c1",
        isError: true,
        output: {
          stdout: "",
          stderr: "",
          result: null,
          status: "error",
          errorName: "ValueError",
          errorValue: "x",
          traceback: ["Traceback (most recent call last):", "ValueError: x"],
          durationMs: 9,
          output: "ValueError: x",
        },
      },
    },
    state,
  );
  const end = ended.find((e) => e.type === "tool_execution_end");
  assert.ok(end);
  assert.equal(end?.isError, true);
  const result = end?.result as {
    content?: Array<{ type?: string; text?: string }>;
    details?: {
      status?: string;
      durationMs?: number;
      errorEname?: string;
      error?: { ename?: string; evalue?: string };
    };
  };
  assert.equal(result.content?.[0]?.text, "ValueError: x");
  assert.equal(result.details?.status, "error");
  assert.equal(result.details?.durationMs, 9);
  assert.equal(result.details?.errorEname, "ValueError");
  assert.equal(result.details?.error?.ename, "ValueError");
  assert.equal(result.details?.error?.evalue, "x");
});

test("ipython success output preserves durationMs under details", () => {
  const state = createStreamState();
  const ended = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "i2",
      item: {
        type: "toolResult",
        callId: "c1",
        isError: false,
        output: {
          stdout: "PRIME_OK\n",
          status: "ok",
          durationMs: 3,
        },
      },
    },
    state,
  );
  const end = ended.find((e) => e.type === "tool_execution_end");
  const result = end?.result as {
    content?: Array<{ text?: string }>;
    details?: { status?: string; durationMs?: number; stdout?: string };
  };
  assert.equal(result.content?.[0]?.text, "PRIME_OK\n");
  assert.equal(result.details?.status, "ok");
  assert.equal(result.details?.durationMs, 3);
  assert.equal(result.details?.stdout, "PRIME_OK\n");
});

test("InteractiveMode-shaped ipython Json output passes through content+details", () => {
  const state = createStreamState();
  const ended = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "i2",
      item: {
        type: "toolResult",
        callId: "c1",
        isError: false,
        output: {
          content: [{ type: "text", text: "4" }],
          details: { status: "ok", durationMs: 1, result: "4" },
        },
      },
    },
    state,
  );
  const end = ended.find((e) => e.type === "tool_execution_end");
  const result = end?.result as {
    content?: Array<{ text?: string }>;
    details?: { status?: string; durationMs?: number; result?: string };
  };
  assert.equal(result.content?.[0]?.text, "4");
  assert.equal(result.details?.status, "ok");
  assert.equal(result.details?.durationMs, 1);
  assert.equal(result.details?.result, "4");
});

test("toolCall inputDelta streams args via tool_args_update (not result)", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/started",
    {
      id: "item_1",
      item: {
        type: "toolCall",
        callId: "c1",
        toolName: "ipython",
        arguments: {},
      },
    },
    state,
  );

  const events = nativeNotificationToPiEvents(
    "item/toolCall/inputDelta",
    {
      itemId: "item_1",
      delta: JSON.stringify({ tool_use_id: "c1", partial_json: '{"code":"print(1' }),
    },
    state,
  );
  assert.equal(events[0]?.type, "tool_args_update");
  assert.equal(events[0]?.toolCallId, "c1");
  const args = events[0]?.args as { code?: string };
  assert.equal(args.code, "print(1");

  const status = nativeNotificationToPiEvents(
    "tool_call/status_updated",
    { toolCallId: "c1", status: "in_progress" },
    state,
  );
  assert.ok(status.some((e) => e.type === "tool_args_complete"));
  assert.ok(status.some((e) => e.type === "tool_execution_start"));
});

test("exec_command maps to bash pending with $ command args", () => {
  const state = createStreamState();
  const events = nativeNotificationToPiEvents(
    "item/started",
    {
      id: "i1",
      item: {
        type: "commandExecution",
        callId: "c1",
        command: "echo hi",
        input: { cmd: "echo hi" },
      },
    },
    state,
  );
  const pending = events.find((e) => e.type === "tool_pending");
  assert.equal(pending?.toolName, "bash");
  assert.equal((pending?.args as { command?: string })?.command, "echo hi");
});

test("restore projects user/assistant items to AgentMessage", () => {
  const messages = nativeItemsToAgentMessages([
    {
      id: "1",
      createdAt: "2026-01-01T00:00:00Z",
      item: { type: "userMessage", content: [{ type: "text", text: "hi" }] },
    },
    {
      id: "2",
      createdAt: "2026-01-01T00:00:01Z",
      item: { type: "assistantMessage", text: "hello" },
    },
  ]);
  assert.equal(messages.length, 2);
  assert.equal(messages[0]?.role, "user");
  assert.equal(messages[1]?.role, "assistant");
});

test("command/exec outputDelta+exited project bash_start/output/end", () => {
  const state = createStreamState();
  const chunk = Buffer.from("hello\n", "utf8").toString("base64");
  const first = nativeNotificationToPiEvents(
    "command/exec/outputDelta",
    { processId: "proc_1", deltaBase64: chunk },
    state,
  );
  assert.equal(first[0]?.type, "bash_start");
  assert.equal(first[0]?.runId, "proc_1");
  assert.equal(first[1]?.type, "bash_output");
  assert.equal(first[1]?.chunk, "hello\n");

  const second = nativeNotificationToPiEvents(
    "command/exec/outputDelta",
    { processId: "proc_1", deltaBase64: Buffer.from("more", "utf8").toString("base64") },
    state,
  );
  assert.equal(second.length, 1);
  assert.equal(second[0]?.type, "bash_output");

  const ended = nativeNotificationToPiEvents(
    "command/exec/exited",
    { processId: "proc_1", exitCode: 0 },
    state,
  );
  assert.deepEqual(ended, [
    {
      type: "bash_end",
      exitCode: 0,
      cancelled: false,
      truncated: false,
      runId: "proc_1",
    },
  ]);
});

test("command/exec outputDelta does not emit second bash_start when user UI is mounted", () => {
  const state = createStreamState();
  state.userBashUiMounted = true;
  const chunk = Buffer.from("hello\n", "utf8").toString("base64");
  const first = nativeNotificationToPiEvents(
    "command/exec/outputDelta",
    { processId: "proc_local", deltaBase64: chunk },
    state,
  );
  assert.equal(first.length, 1);
  assert.equal(first[0]?.type, "bash_output");
  assert.equal(first[0]?.chunk, "hello\n");
});

test("context compaction maps to compaction_start/end", () => {
  const state = createStreamState();
  assert.deepEqual(
    nativeNotificationToPiEvents(
      "context/compactionStarted",
      { sessionId: "s1", turnId: "t1", trigger: "manual" },
      state,
    ),
    [{ type: "compaction_start", reason: "manual" }],
  );
  const completed = nativeNotificationToPiEvents(
    "context/compactionCompleted",
    { sessionId: "s1", turnId: "t1", itemId: "item_c" },
    state,
  );
  assert.equal(completed[0]?.type, "compaction_end");
  assert.equal(completed[0]?.aborted, false);

  const failed = nativeNotificationToPiEvents(
    "context/compactionFailed",
    { sessionId: "s1", message: "boom" },
    state,
  );
  assert.equal(failed[0]?.type, "compaction_end");
  assert.equal(failed[0]?.errorMessage, "boom");
  assert.equal(failed[0]?.errorSeverity, "error");
});

test("session/goal/* projects goal_update", () => {
  const state = createStreamState();
  const created = nativeNotificationToPiEvents(
    "session/goal/created",
    {
      goal: {
        goalId: "g1",
        objective: "Ship it",
        status: "active",
        tokensUsed: 10,
        timeUsedSeconds: 1,
        continuationsUsed: 0,
      },
    },
    state,
  );
  assert.equal(created[0]?.type, "goal_update");
  const goal = created[0]?.goal as { status?: string; objective?: string; active?: boolean };
  assert.equal(goal.status, "active");
  assert.equal(goal.objective, "Ship it");
  assert.equal(goal.active, true);

  const statusChanged = nativeNotificationToPiEvents(
    "session/goal/statusChanged",
    { sessionId: "s1", goalId: "g1", status: "completed" },
    state,
  );
  assert.equal(statusChanged[0]?.type, "goal_update");
  assert.equal((statusChanged[0]?.goal as { status?: string }).status, "complete");
  assert.equal((statusChanged[0]?.goal as { active?: boolean }).active, false);

  const updated = nativeNotificationToPiEvents(
    "session/goal/updated",
    {
      goal: {
        goalId: "g1",
        objective: "Ship it",
        status: "completed",
        tokensUsed: 10,
        timeUsedSeconds: 1,
        continuationsUsed: 0,
      },
    },
    state,
  );
  assert.equal((updated[0]?.goal as { status?: string }).status, "complete");

  const cleared = nativeNotificationToPiEvents(
    "session/goal/cleared",
    { sessionId: "s1", goalId: "g1" },
    state,
  );
  assert.equal(cleared[0]?.type, "goal_update");
  assert.equal((cleared[0]?.goal as { status?: string }).status, "idle");
});

test("provider/authStale projects auth_stale", () => {
  const state = createStreamState();
  assert.deepEqual(
    nativeNotificationToPiEvents(
      "provider/authStale",
      { providerId: "openai", reason: "expired" },
      state,
    ),
    [
      {
        type: "auth_stale",
        provider: "openai",
        sourceTokens: undefined,
      },
    ],
  );
});

test("queue/updated projects session_action_update from Native queue entries", () => {
  const state = createStreamState();
  const events = nativeNotificationToPiEvents(
    "queue/updated",
    {
      sessionId: "s1",
      change: "added",
      queueItemId: "q1",
      queue: [
        { queueItemId: "q1", position: 0, preview: "follow me", input: [{ type: "text", text: "follow me" }] },
      ],
    },
    state,
  );
  assert.equal(events[0]?.type, "session_action_update");
  const actions = events[0]?.actions as { followUps?: string[]; queuedCount?: number };
  assert.deepEqual(actions.followUps, ["follow me"]);
  assert.equal(actions.queuedCount, 1);
});

test("model/queryRetrying Scheduled starts countdown; Resumed clears it", () => {
  const state = createStreamState();
  const scheduled = nativeNotificationToPiEvents(
    "model/queryRetrying",
    {
      attempt: 1,
      maxAttempts: 5,
      nextDelayMs: 1500,
      phase: "scheduled",
      error: { message: "temporary" },
    },
    state,
  );
  assert.deepEqual(scheduled, [
    {
      type: "auto_retry_start",
      attempt: 1,
      maxAttempts: 5,
      delayMs: 1500,
      errorMessage: "temporary",
    },
  ]);

  const resumed = nativeNotificationToPiEvents(
    "model/queryRetrying",
    {
      attempt: 1,
      maxAttempts: 5,
      nextDelayMs: 0,
      phase: "resumed",
      error: { message: "temporary" },
    },
    state,
  );
  assert.deepEqual(resumed, [
    {
      type: "auto_retry_end",
      success: true,
      attempt: 1,
    },
  ]);
});

test("plan item completes the update_plan tool row with a checklist", () => {
  const state = createStreamState();
  // Live sequence: item/started (Plan, empty) → status_updated → item/completed.
  const started = nativeNotificationToPiEvents(
    "item/started",
    {
      item: {
        id: "item_plan_1",
        item: { type: "plan", callId: "call-plan-1", entries: [] },
      },
    },
    state,
  );
  assert.deepEqual(started.map((e) => e.type), ["tool_pending"]);
  assert.equal(started[0]?.toolName, "update_plan");

  nativeNotificationToPiEvents(
    "tool_call/status_updated",
    { toolCallId: "call-plan-1", status: "in_progress" },
    state,
  );

  const completed = nativeNotificationToPiEvents(
    "item/completed",
    {
      item: {
        id: "item_plan_1",
        item: {
          type: "plan",
          callId: "call-plan-1",
          entries: [
            { step: "Verify-R8-PLAN-CREATE", status: "inProgress" },
            { step: "Probe-R8-PLAN-UPDATE", status: "pending" },
          ],
        },
      },
    },
    state,
  );
  const end = completed.find((e) => e.type === "tool_execution_end");
  assert.ok(end, "plan completion must close the tool row");
  assert.equal(end.toolName, "update_plan");
  const text = (end.result as { content: Array<{ text: string }> }).content[0]?.text;
  assert.match(text ?? "", /\[~\] Verify-R8-PLAN-CREATE/);
  assert.match(text ?? "", /\[ \] Probe-R8-PLAN-UPDATE/);
  // No pending/running rows may remain open for this call.
  assert.equal(state.tools.pendingCalls.has("call-plan-1"), false);
  assert.equal(state.tools.startedCalls.has("call-plan-1"), false);
});

test("plan completion without callId correlates via the itemId mapping", () => {
  const state = createStreamState();
  nativeNotificationToPiEvents(
    "item/started",
    { item: { id: "item_plan_9", item: { type: "plan", callId: "call-plan-9", entries: [] } } },
    state,
  );
  // Completion arrives without callId (older server payload).
  const completed = nativeNotificationToPiEvents(
    "item/completed",
    {
      item: {
        id: "item_plan_9",
        item: { type: "plan", entries: [{ step: "only-step", status: "completed" }] },
      },
    },
    state,
  );
  const end = completed.find((e) => e.type === "tool_execution_end");
  assert.ok(end, "itemId fallback must still close the row");
  assert.equal(end.toolCallId, "call-plan-9");
});

test("live Python kernel restore item emits the injected restored-state notice", () => {
  const events = nativeNotificationToPiEvents(
    "item/completed",
    {
      id: "kernel-restored",
      item: {
        type: "warning",
        code: "ipythonKernelStateRestored",
        message: "Restored Python kernel state",
        retryable: false,
      },
    },
    createStreamState(),
  );
  assert.equal(events.length, 1);
  assert.equal(events[0]?.type, "message_start");
  const message = events[0]?.message as {
    role?: string;
    customType?: string;
    details?: { restored?: boolean };
  };
  assert.equal(message.role, "custom");
  assert.equal(message.customType, "ipython_state_restored");
  assert.deepEqual(message.details, { restored: true });
});
