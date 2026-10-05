/**
 * Convert Native ServerNotification methods into AgentConnectionSessionEvent[].
 * Projection stays inside the adapter — InteractiveMode never sees Native method names.
 */

import {
  nativeGoalToAgentGoal,
  occupancyToContextUsage,
  pythonKernelStateMessageFromNativeItem,
} from "./messages.js";
import {
  bashResultFromCommandExecution,
  createNativeToPiToolState,
  displayToolName,
  editResultFromFileChange,
  planResultFromItem,
  projectCommandExecutionOutputDelta,
  projectPendingToolsAborted,
  projectToolCallInputDelta,
  projectToolExecutionEnd,
  projectToolItemStarted,
  projectToolStatusUpdated,
  shapeToolResultOutput,
  type NativeToPiToolState,
} from "./tools.js";

export type StreamState = {
  openAssistantItem: string | null;
  openReasoningItem: string | null;
  imAssistantStreamOpen: boolean;
  /** Local abort chrome already painted (skip duplicate abort on turn/completed). */
  localAbortPainted: boolean;
  assistantTextByItem: Map<string, string>;
  reasoningTextByItem: Map<string, string>;
  /** @deprecated prefer tools.toolNameByCall — kept for call sites that still read it. */
  toolNameByCall: Map<string, string>;
  tools: NativeToPiToolState;
  /** processId values that already emitted bash_start for command/exec/*. */
  activeBashRuns: Set<string>;
  /**
   * Local `!` bash already mounted a BashExecutionComponent. First
   * `command/exec/outputDelta` must not synthesize a second bash_start
   * (processId is unknown until task/start returns).
   */
  userBashUiMounted: boolean;
};

export function createStreamState(): StreamState {
  const tools = createNativeToPiToolState();
  return {
    openAssistantItem: null,
    openReasoningItem: null,
    imAssistantStreamOpen: false,
    localAbortPainted: false,
    assistantTextByItem: new Map(),
    reasoningTextByItem: new Map(),
    toolNameByCall: tools.toolNameByCall,
    tools,
    activeBashRuns: new Set(),
    userBashUiMounted: false,
  };
}

const STREAM_METHODS = new Set([
  "turn/started",
  "turn/resumed",
  "turn/completed",
  "item/started",
  "item/updated",
  "item/completed",
  "item/assistantMessage/delta",
  "item/reasoning/delta",
  "item/commandExecution/outputDelta",
  "item/toolCall/inputDelta",
  "item/plan/delta",
]);

export function isNativeStreamMethod(method: string): boolean {
  return STREAM_METHODS.has(String(method ?? ""));
}

function emptyUsage() {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
  };
}

function assistantMessageParts(text: string, thinking: string) {
  const content: Array<Record<string, unknown>> = [];
  if (thinking) content.push({ type: "thinking", thinking });
  content.push({ type: "text", text: text ?? "" });
  return {
    role: "assistant",
    content,
    timestamp: Date.now(),
    stopReason: null,
    errorMessage: null,
    usage: emptyUsage(),
  };
}

/** Local abort chrome when Ctrl+C clears the turn before turn/completed arrives. */
export function abortUiSessionEvents(
  state?: StreamState,
): Array<Record<string, unknown>> {
  const events: Array<Record<string, unknown>> = [];
  if (state) {
    events.push(...projectPendingToolsAborted(state.tools));
    events.push(...closeImAssistantStreamIfOpen(state, { stopReason: "aborted" }));
  }
  if (!events.some((e) => e.type === "message_end")) {
    events.push({
      type: "message_end",
      message: {
        ...assistantMessageParts("", ""),
        stopReason: "aborted",
      },
    });
  }
  events.push({ type: "agent_end" });
  return events;
}

function liveAssistantMessage(state: StreamState) {
  const text = state.openAssistantItem
    ? (state.assistantTextByItem.get(state.openAssistantItem) ?? "")
    : "";
  const thinking = state.openReasoningItem
    ? (state.reasoningTextByItem.get(state.openReasoningItem) ?? "")
    : "";
  return assistantMessageParts(text, thinking);
}

function closeImAssistantStreamIfOpen(
  state: StreamState,
  options?: { stopReason?: string | null; errorMessage?: string | null },
): Array<Record<string, unknown>> {
  if (!state.imAssistantStreamOpen) return [];
  state.imAssistantStreamOpen = false;
  const message = {
    ...liveAssistantMessage(state),
    ...(options?.stopReason != null ? { stopReason: options.stopReason } : {}),
    ...(options?.errorMessage != null ? { errorMessage: options.errorMessage } : {}),
  };
  return [
    {
      type: "message_end",
      message,
    },
  ];
}

function turnTerminalStopReason(status: string): {
  stopReason: string | null;
  errorMessage: string | null;
} {
  const normalized = status.toLowerCase();
  if (normalized === "interrupted") {
    return { stopReason: "aborted", errorMessage: null };
  }
  if (normalized === "failed") {
    return { stopReason: "error", errorMessage: null };
  }
  return { stopReason: null, errorMessage: null };
}

/** Prefer turn.error.message; never stringify the whole error object. */
function extractTurnErrorMessage(turn: Record<string, unknown>): string {
  const err = turn.error;
  if (typeof err === "string") {
    const trimmed = err.trim();
    return trimmed.length > 0 ? trimmed : "Error";
  }
  if (err && typeof err === "object") {
    const message = String((err as { message?: unknown }).message ?? "").trim();
    if (message.length > 0) return message;
  }
  return "Error";
}

function userMessageFromNative(content: unknown) {
  const parts = Array.isArray(content) ? content : [];
  const out: Array<Record<string, unknown>> = [];
  for (const c of parts) {
    if (!c || typeof c !== "object") continue;
    const part = c as Record<string, unknown>;
    if (
      part.type === "text" ||
      (part.text != null && part.type !== "localImage" && part.type !== "image")
    ) {
      const text = String(part.text ?? "");
      if (text) out.push({ type: "text", text });
      continue;
    }
    if (part.type === "localImage" || part.type === "image") {
      const label =
        part.path != null
          ? `[image:${String(part.path)}]`
          : part.uri != null
            ? "[image]"
            : "[image]";
      out.push({ type: "text", text: label });
    }
  }
  if (out.length === 0) out.push({ type: "text", text: "" });
  return {
    role: "user",
    content: out,
    timestamp: Date.now(),
  };
}

function decodeBase64Utf8(b64: string): string {
  if (!b64) return "";
  try {
    return Buffer.from(b64, "base64").toString("utf8");
  } catch {
    return "";
  }
}

function compactionReason(trigger: unknown): "manual" | "threshold" | "overflow" | "requested" {
  const raw = String(trigger ?? "").toLowerCase();
  if (raw === "manual") return "manual";
  if (raw === "agentrequested" || raw === "agent_requested" || raw === "requested") return "requested";
  if (raw === "providerretry" || raw === "provider_retry" || raw === "overflow") return "overflow";
  return "threshold";
}

function queuePreviewText(entry: Record<string, unknown>): string {
  const preview = String(entry.preview ?? "").trim();
  if (preview) return preview;
  const input = Array.isArray(entry.input) ? entry.input : [];
  const parts: string[] = [];
  for (const part of input) {
    if (!part || typeof part !== "object") continue;
    const text = (part as { text?: unknown }).text;
    if (text != null) parts.push(String(text));
  }
  return parts.join("\n").trim();
}

function sessionActionsFromQueue(params: Record<string, unknown>): Record<string, unknown> {
  if (Array.isArray(params.steering) || Array.isArray(params.followUp) || Array.isArray(params.followUps)) {
    const steering = Array.isArray(params.steering)
      ? params.steering.map((x) => String((x as { text?: string })?.text ?? x))
      : [];
    const followUps = Array.isArray(params.followUp ?? params.followUps)
      ? (params.followUp ?? params.followUps as unknown[]).map((x) =>
          String((x as { text?: string })?.text ?? x),
        )
      : [];
    return {
      queuedCount: steering.length + followUps.length,
      steering,
      followUps,
    };
  }
  const queue = Array.isArray(params.queue) ? params.queue : [];
  const followUps: string[] = [];
  for (const entry of queue) {
    if (!entry || typeof entry !== "object") continue;
    const text = queuePreviewText(entry as Record<string, unknown>);
    if (text) followUps.push(text);
  }
  return {
    queuedCount: followUps.length,
    steering: [] as string[],
    followUps,
  };
}

function contextUsageEvent(params: Record<string, unknown>): Record<string, unknown> | null {
  const occupancy = params.occupancy ?? params.usage ?? params;
  const usage = occupancyToContextUsage(occupancy);
  if (!usage) {
    const query =
      occupancy && typeof occupancy === "object"
        ? ((occupancy as Record<string, unknown>).query as Record<string, unknown> | undefined)
        : undefined;
    const tokens = Number(query?.totalTokens ?? params.lastQueryInputTokens ?? 0);
    const contextWindow = Number(params.contextWindow ?? params.context_window ?? 0);
    if (!tokens && !contextWindow) return null;
    const percent =
      contextWindow > 0 ? Math.min(100, Math.round((tokens / contextWindow) * 1000) / 10) : 0;
    return {
      type: "context_usage_update",
      contextUsage: { tokens, contextWindow: contextWindow || 1, percent },
    };
  }
  return { type: "context_usage_update", contextUsage: usage };
}

function rlmChildFromAgent(
  params: Record<string, unknown>,
  status: "queued" | "running" | "done" | "error" | "cancelled",
  extras: Record<string, unknown> = {},
): Record<string, unknown> {
  const itemId = String(params.itemId ?? params.item_id ?? "");
  const agentSessionId = String(params.agentSessionId ?? params.agent_session_id ?? "");
  return {
    type: "rlm_child_update",
    child: {
      id: itemId || agentSessionId || "agent",
      activeSessionId: agentSessionId || undefined,
      label: String(params.summary ?? params.label ?? (itemId || "agent")),
      status,
      sessionDir: String(params.sessionDir ?? params.session_dir ?? ""),
      recap: typeof params.summary === "string" ? params.summary : undefined,
      error: typeof params.error === "string" ? params.error : undefined,
      ...extras,
    },
  };
}

function projectItemStarted(
  envelope: Record<string, unknown>,
  state: StreamState,
): Array<Record<string, unknown>> {
  const item = (envelope.item ?? envelope) as Record<string, unknown>;
  const events: Array<Record<string, unknown>> = [];
  if (!item || typeof item !== "object") return events;
  const key = String(envelope.id ?? "");
  const type = item.type;

  if (type === "userMessage") {
    events.push(...closeImAssistantStreamIfOpen(state));
    events.push({
      type: "message_start",
      message: userMessageFromNative(item.content),
    });
    events.push({
      type: "message_end",
      message: userMessageFromNative(item.content),
    });
  } else if (type === "assistantMessage") {
    state.openAssistantItem = key;
    state.assistantTextByItem.set(key, String(item.text ?? ""));
    if (!state.imAssistantStreamOpen) {
      state.imAssistantStreamOpen = true;
      events.push({
        type: "message_start",
        message: liveAssistantMessage(state),
      });
    }
  } else if (type === "reasoning") {
    state.openReasoningItem = key;
    state.reasoningTextByItem.set(key, String(item.text ?? ""));
    if (!state.imAssistantStreamOpen) {
      state.imAssistantStreamOpen = true;
      events.push({
        type: "message_start",
        message: liveAssistantMessage(state),
      });
    }
    events.push({
      type: "message_update",
      message: liveAssistantMessage(state),
      assistantMessageEvent: { type: "thinking_start" },
    });
  } else if (type === "toolCall" || type === "tool_call" || type === "commandExecution" || type === "fileChange" || type === "hostedToolCall" || type === "hosted_tool_call" || type === "plan") {
    events.push(...closeImAssistantStreamIfOpen(state));
    events.push(...projectToolItemStarted(envelope, item, state.tools));
  } else if (type === "contextCompaction") {
    events.push({
      type: "compaction_start",
      reason: compactionReason(item.trigger),
    });
  }

  return events;
}

function projectItemCompleted(
  envelope: Record<string, unknown>,
  state: StreamState,
): Array<Record<string, unknown>> {
  const item = (envelope.item ?? envelope) as Record<string, unknown>;
  const events: Array<Record<string, unknown>> = [];
  if (!item || typeof item !== "object") return events;
  const key = String(envelope.id ?? "");
  const type = item.type;

  if (type === "assistantMessage") {
    const existing = state.assistantTextByItem.get(key) ?? "";
    // Delta-less clients (proxy transport, mid-turn attach) never populated
    // the accumulator — the completed item carries the full text.
    const text = existing.length > 0 ? existing : String(item.text ?? "");
    state.assistantTextByItem.delete(key);
    if (state.openAssistantItem === key) state.openAssistantItem = null;
    const thinking = state.openReasoningItem
      ? (state.reasoningTextByItem.get(state.openReasoningItem) ?? "")
      : "";
    state.imAssistantStreamOpen = false;
    state.openReasoningItem = null;
    events.push({
      type: "message_end",
      message: assistantMessageParts(text, thinking),
    });
  } else if (type === "reasoning") {
    const existing = state.reasoningTextByItem.get(key);
    if ((!existing || existing.length === 0) && item.text) {
      state.reasoningTextByItem.set(key, String(item.text));
    }
    events.push({
      type: "message_update",
      message: liveAssistantMessage(state),
      assistantMessageEvent: { type: "thinking_end" },
    });
  } else if (type === "toolResult" || type === "tool_result") {
    const callId = String(item.callId ?? item.call_id ?? "");
    const resultValue = shapeToolResultOutput(item.output, item.displayContent ?? item.display_content);
    events.push(
      ...projectToolExecutionEnd(callId, resultValue, Boolean(item.isError ?? item.is_error), state.tools),
    );
    const details = (resultValue as { details?: { sentAgentMessages?: unknown[] } })?.details;
    const messages = details?.sentAgentMessages;
    if (Array.isArray(messages)) {
      for (const message of messages) {
        events.push({
          type: "ipython_sent_agent_message",
          toolCallId: callId,
          message,
        });
      }
    }
  } else if (type === "hostedToolCall" || type === "hosted_tool_call") {
    const callId = String(item.callId ?? item.call_id ?? item.id ?? "");
    const rawName = String(item.toolName ?? item.tool_name ?? item.name ?? "web_search");
    const toolName = displayToolName(rawName);
    if (!state.tools.toolNameByCall.has(callId)) {
      state.tools.toolNameByCall.set(callId, toolName);
    }
    const hasOutput = item.output != null || item.displayContent != null || item.display_content != null;
    if (hasOutput) {
      const shaped = shapeToolResultOutput(item.output, item.displayContent ?? item.display_content);
      events.push(
        ...projectToolExecutionEnd(callId, shaped, Boolean(item.isError ?? item.is_error), state.tools),
      );
    }
  } else if (type === "commandExecution") {
    const callId = String(item.callId ?? item.call_id ?? "");
    events.push(
      ...projectToolExecutionEnd(
        callId,
        bashResultFromCommandExecution(item),
        Number(item.exitCode ?? item.exit_code ?? 0) !== 0,
        state.tools,
      ),
    );
  } else if (type === "fileChange") {
    const callId = String(item.callId ?? item.call_id ?? "");
    const shaped = editResultFromFileChange(item);
    // Keep tool name `edit` so edit renderer + getToolFileChanges apply.
    if (!state.tools.toolNameByCall.has(callId)) {
      state.tools.toolNameByCall.set(callId, "edit");
    }
    events.push(...projectToolExecutionEnd(callId, shaped, false, state.tools));
  } else if (type === "plan") {
    // Completed update_plan call: close the tool row with the checklist.
    // Correlate via the item's callId, falling back to the itemId mapping
    // (older servers / plan items without call_id).
    const itemId = String(envelope.id ?? "");
    const callId =
      String(item.callId ?? item.call_id ?? "") ||
      state.tools.callIdByItemId.get(itemId) ||
      itemId;
    if (callId) {
      if (!state.tools.toolNameByCall.has(callId)) {
        state.tools.toolNameByCall.set(callId, "update_plan");
      }
      events.push(
        ...projectToolExecutionEnd(callId, planResultFromItem(item), false, state.tools),
      );
    }
  } else if (type === "contextCompaction") {
    const failed = String(item.status ?? envelope.state ?? "").toLowerCase() === "failed";
    const rawMessage = String(
      item.error ?? item.message ?? item.summary ?? (failed ? "Compaction failed" : ""),
    );
    const tooShort = /too short|skip|not enough/i.test(rawMessage);
    // InteractiveMode only surfaces manual compaction errors/warnings.
    const reason =
      tooShort || failed ? ("manual" as const) : compactionReason(item.trigger);
    events.push({
      type: "compaction_end",
      reason,
      result: failed || tooShort ? undefined : item,
      aborted: false,
      willRetry: false,
      errorMessage:
        failed || tooShort
          ? tooShort
            ? "Session is too short to compact — try again once it grows"
            : rawMessage
          : undefined,
      errorSeverity: tooShort ? "warning" : failed ? "error" : undefined,
    });
  } else if (type === "warning") {
    const kernelStateMessage = pythonKernelStateMessageFromNativeItem(item);
    if (kernelStateMessage) {
      events.push({ type: "message_start", message: kernelStateMessage });
    } else {
      // Live server-pushed Warning items (e.g. rlmKernelUnfenced — OS fence
      // downgrade, design doc §5.3): paint as error chrome. Persisted/replay
      // path renders the same shape in native-to-pi/messages.ts.
      const warningMessage = String(item.message ?? "").trim();
      if (warningMessage && !state.imAssistantStreamOpen) {
        events.push({
          type: "message_end",
          message: {
            ...assistantMessageParts("", ""),
            stopReason: "error",
            errorMessage: warningMessage,
          },
        });
      }
    }
  }

  return events;
}

function projectItemUpdated(
  envelope: Record<string, unknown>,
  state: StreamState,
): Array<Record<string, unknown>> {
  const item = (envelope.item ?? envelope) as Record<string, unknown>;
  if (!item || typeof item !== "object") return [];
  const key = String(envelope.id ?? "");
  const type = item.type;

  if (type === "assistantMessage") {
    const text = String(item.text ?? "");
    state.openAssistantItem = key;
    state.assistantTextByItem.set(key, text);
    const events: Array<Record<string, unknown>> = [];
    if (!state.imAssistantStreamOpen) {
      state.imAssistantStreamOpen = true;
      events.push({ type: "message_start", message: liveAssistantMessage(state) });
    }
    events.push({
      type: "message_update",
      message: liveAssistantMessage(state),
      assistantMessageEvent: { type: "text_delta", delta: "" },
    });
    return events;
  }
  if (type === "reasoning") {
    state.openReasoningItem = key;
    state.reasoningTextByItem.set(key, String(item.text ?? ""));
    return [
      {
        type: "message_update",
        message: liveAssistantMessage(state),
        assistantMessageEvent: { type: "thinking_delta", delta: "" },
      },
    ];
  }
  if (type === "plan") {
    return [
      {
        type: "session_lifecycle",
        action: "plan_updated",
        item,
      },
    ];
  }
  return [
    {
      type: "session_lifecycle",
      action: "item_updated",
      itemType: type,
      itemId: key,
    },
  ];
}

function itemEnvelopeFromParams(p: Record<string, unknown>): Record<string, unknown> {
  // Wire: { item: ItemEnvelope { id, item: Item } }
  // Flat adapter/tests: { id, item: Item }
  const nested = p.item;
  if (nested != null && typeof nested === "object") {
    const record = nested as Record<string, unknown>;
    if (typeof record.type === "string") {
      return p;
    }
    return record;
  }
  return p;
}

/**
 * The approval id a notification resolves, if any: an Approval item that has
 * reached a decision. When one client answers a fanned-out permission
 * request, every other client holding an open dialog for the same approval
 * needs to dismiss it (the server takes the first answer and never responds
 * to the losers' requests).
 */
export function approvalResolutionFromNotification(method: string, params: unknown): string | null {
  if (method !== "item/completed" && method !== "item/updated") {
    return null;
  }
  const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;
  const envelope = itemEnvelopeFromParams(p);
  const item = (envelope.item ?? envelope) as
    | { type?: unknown; approvalId?: unknown; approval_id?: unknown; decision?: unknown }
    | null;
  if (!item || typeof item !== "object" || String(item.type) !== "approval" || item.decision == null) {
    return null;
  }
  const approvalId = item.approvalId ?? item.approval_id;
  return typeof approvalId === "string" && approvalId ? approvalId : null;
}

/** Expand one Native notification into zero or more InteractiveMode session events. */
export function nativeNotificationToPiEvents(
  method: string,
  params: unknown,
  state: StreamState,
): Array<Record<string, unknown>> {
  const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;

  // ── Turn / item stream ──
  if (method === "turn/started" || method === "turn/resumed") {
    return [{ type: "agent_start" }];
  }

  if (method === "turn/completed") {
    const turn = ((p.turn ?? p) as Record<string, unknown>) ?? {};
    const status = String(turn.status ?? "");
    const { stopReason } = turnTerminalStopReason(status);
    const failedMessage = stopReason === "error" ? extractTurnErrorMessage(turn) : null;
    const events: Array<Record<string, unknown>> = [];
    if (stopReason === "aborted" || stopReason === "error") {
      events.push(...projectPendingToolsAborted(state.tools));
    }
    events.push(
      ...closeImAssistantStreamIfOpen(state, {
        stopReason,
        errorMessage: stopReason === "error" ? failedMessage : null,
      }),
    );
    // Interrupted / failed before any assistant tokens still needs visible chrome.
    if (
      (stopReason === "aborted" || stopReason === "error") &&
      !events.some((e) => e.type === "message_end")
    ) {
      events.push({
        type: "message_end",
        message: {
          ...assistantMessageParts("", ""),
          stopReason,
          errorMessage: stopReason === "error" ? failedMessage : null,
        },
      });
    }
    events.push({ type: "agent_end" });
    state.openAssistantItem = null;
    state.openReasoningItem = null;
    return events;
  }

  if (method === "turn/statusChanged") {
    const status = String(p.status ?? "").toLowerCase();
    // Failed status has no turn.error here — defer stream close + Error chrome to
    // turn/completed so a race cannot wipe the provider message with null.
    if (status === "failed") {
      return [{ type: "agent_end" }];
    }
    if (status === "completed" || status === "interrupted") {
      const { stopReason } = turnTerminalStopReason(status);
      const events: Array<Record<string, unknown>> = [];
      if (stopReason === "aborted") {
        events.push(...projectPendingToolsAborted(state.tools));
      }
      events.push(
        ...closeImAssistantStreamIfOpen(state, {
          stopReason,
          errorMessage: null,
        }),
      );
      if (stopReason === "aborted" && !events.some((e) => e.type === "message_end")) {
        events.push({
          type: "message_end",
          message: {
            ...assistantMessageParts("", ""),
            stopReason: "aborted",
            errorMessage: null,
          },
        });
      }
      events.push({ type: "agent_end" });
      return events;
    }
    if (status === "inprogress" || status === "in_progress" || status === "active") {
      return [{ type: "agent_start" }];
    }
    return [{ type: "session_lifecycle", action: "turn_status", status: p.status, turnId: p.turnId }];
  }

  if (method === "item/started") {
    return projectItemStarted(itemEnvelopeFromParams(p), state);
  }

  if (method === "item/updated") {
    return projectItemUpdated(itemEnvelopeFromParams(p), state);
  }

  if (method === "item/completed") {
    return projectItemCompleted(itemEnvelopeFromParams(p), state);
  }

  if (method === "item/assistantMessage/delta") {
    const itemId = String(p.itemId ?? p.item_id ?? state.openAssistantItem ?? "");
    const delta = String(p.delta ?? p.text ?? "");
    if (!itemId) return [];
    state.openAssistantItem = itemId;
    const prev = state.assistantTextByItem.get(itemId) ?? "";
    state.assistantTextByItem.set(itemId, prev + delta);
    const events: Array<Record<string, unknown>> = [];
    if (!state.imAssistantStreamOpen) {
      state.imAssistantStreamOpen = true;
      events.push({ type: "message_start", message: liveAssistantMessage(state) });
    }
    events.push({
      type: "message_update",
      message: liveAssistantMessage(state),
      assistantMessageEvent: { type: "text_delta", delta },
    });
    return events;
  }

  if (method === "item/reasoning/delta") {
    const itemId = String(p.itemId ?? p.item_id ?? state.openReasoningItem ?? "");
    const delta = String(p.delta ?? p.text ?? "");
    if (!itemId) return [];
    state.openReasoningItem = itemId;
    const prev = state.reasoningTextByItem.get(itemId) ?? "";
    state.reasoningTextByItem.set(itemId, prev + delta);
    const events: Array<Record<string, unknown>> = [];
    if (!state.imAssistantStreamOpen) {
      state.imAssistantStreamOpen = true;
      events.push({ type: "message_start", message: liveAssistantMessage(state) });
    }
    events.push({
      type: "message_update",
      message: liveAssistantMessage(state),
      assistantMessageEvent: { type: "thinking_delta", delta },
    });
    return events;
  }

  if (method === "item/toolCall/inputDelta") {
    return projectToolCallInputDelta(p, state.tools);
  }

  if (method === "item/commandExecution/outputDelta") {
    return projectCommandExecutionOutputDelta(p, state.tools);
  }

  if (method === "item/plan/delta") {
    return [
      {
        type: "session_lifecycle",
        action: "plan_delta",
        itemId: p.itemId ?? p.item_id,
        delta: p.delta ?? p.text,
      },
    ];
  }

  // ── User bash (command/exec) ──
  if (method === "command/exec/outputDelta") {
    const processId = String(p.processId ?? p.process_id ?? "");
    if (!processId) return [];
    const chunk = decodeBase64Utf8(String(p.deltaBase64 ?? p.delta_base64 ?? ""));
    const events: Array<Record<string, unknown>> = [];
    const alreadyStarted =
      state.activeBashRuns.has(processId) || state.userBashUiMounted;
    if (!state.activeBashRuns.has(processId)) {
      state.activeBashRuns.add(processId);
    }
    if (!alreadyStarted) {
      events.push({
        type: "bash_start",
        command: String(p.command ?? ""),
        excludeFromContext: Boolean(p.excludeFromContext ?? p.exclude_from_context ?? true),
        runId: processId,
      });
    }
    if (chunk) events.push({ type: "bash_output", chunk });
    return events;
  }

  if (method === "command/exec/exited") {
    const processId = String(p.processId ?? p.process_id ?? "");
    state.activeBashRuns.delete(processId);
    state.userBashUiMounted = false;
    const exitCode = p.exitCode ?? p.exit_code;
    return [
      {
        type: "bash_end",
        exitCode: exitCode == null ? undefined : Number(exitCode),
        cancelled: false,
        truncated: false,
        runId: processId || undefined,
      },
    ];
  }

  // ── Compaction ──
  if (method === "context/compactionStarted") {
    return [
      {
        type: "compaction_start",
        reason: compactionReason(p.trigger),
      },
    ];
  }

  if (method === "context/compactionCompleted") {
    return [
      {
        type: "compaction_end",
        reason: compactionReason(p.trigger),
        result: {
          summary: "",
          firstKeptEntryId: String(p.itemId ?? p.item_id ?? ""),
          tokensBefore: 0,
          tokensAfter: 0,
        },
        aborted: false,
        willRetry: false,
      },
    ];
  }

  if (method === "context/compactionFailed") {
    const message = String(p.message ?? "Compaction failed");
    const tooShort = /too short|skip|not enough/i.test(message);
    return [
      {
        type: "compaction_end",
        reason: compactionReason(p.trigger),
        result: undefined,
        aborted: false,
        willRetry: false,
        errorMessage: tooShort
          ? "Session is too short to compact — try again once it grows"
          : message,
        errorSeverity: tooShort ? "warning" : "error",
      },
    ];
  }

  // ── Queue ──
  if (method === "queue/updated") {
    return [
      {
        type: "session_action_update",
        actions: sessionActionsFromQueue(p),
      },
    ];
  }

  // ── Goal ──
  if (
    method === "session/goal/created" ||
    method === "session/goal/updated" ||
    method === "session/goal/statusChanged" ||
    method === "session/goal/cleared"
  ) {
    const goalSource =
      method === "session/goal/cleared"
        ? null
        : (p.goal ??
          (method === "session/goal/statusChanged"
            ? {
                goalId: p.goalId ?? p.goal_id,
                status: p.status,
                // Keep tray fields stable when the wire payload is status-only.
                objective: p.objective,
                tokensUsed: p.tokensUsed ?? p.tokens_used,
                timeUsedSeconds: p.timeUsedSeconds ?? p.time_used_seconds,
              }
            : p));
    return [
      {
        type: "goal_update",
        goal: nativeGoalToAgentGoal(goalSource),
      },
    ];
  }

  // ── Usage ──
  if (
    method === "context/usageUpdated" ||
    method === "turn/usage/updated" ||
    method === "session/usage/updated"
  ) {
    const event = contextUsageEvent(p);
    return event ? [event] : [];
  }

  // ── Auth ──
  if (method === "provider/authStale") {
    return [
      {
        type: "auth_stale",
        provider: String(p.providerId ?? p.provider_id ?? p.provider ?? "unknown"),
        sourceTokens: p.sourceTokens ?? p.source_tokens,
      },
    ];
  }

  // ── Agent / task (subagents + background) ──
  if (method === "agent/started") {
    return [rlmChildFromAgent(p, "running")];
  }
  if (method === "agent/progress") {
    return [rlmChildFromAgent(p, "running", { recap: String(p.summary ?? "") })];
  }
  if (method === "agent/completed") {
    const stateRaw = String(p.state ?? "").toLowerCase();
    const status =
      stateRaw === "failed" || stateRaw === "error"
        ? "error"
        : stateRaw === "cancelled" || stateRaw === "interrupted"
          ? "cancelled"
          : "done";
    return [rlmChildFromAgent(p, status)];
  }

  if (method === "task/started") {
    return [
      {
        type: "session_lifecycle",
        action: "task_started",
        itemId: p.itemId ?? p.item_id,
      },
    ];
  }
  if (method === "task/delta") {
    return [
      {
        type: "session_lifecycle",
        action: "task_delta",
        itemId: p.itemId ?? p.item_id,
        delta: p.delta,
        chunkIndex: p.chunkIndex ?? p.chunk_index,
      },
    ];
  }
  if (method === "task/completed" || method === "task/lost") {
    return [
      {
        type: "session_lifecycle",
        action: method === "task/lost" ? "task_lost" : "task_completed",
        itemId: p.itemId ?? p.item_id,
        exitCode: p.exitCode ?? p.exit_code,
      },
    ];
  }

  // ── Session lifecycle / metadata ──
  if (method === "session/metadataUpdated") {
    const session = (p.session ?? p) as Record<string, unknown>;
    const events: Array<Record<string, unknown>> = [
      {
        type: "session_info_changed",
        name: session.title ?? session.name,
      },
    ];
    if (session.goal) {
      events.push({ type: "goal_update", goal: nativeGoalToAgentGoal(session.goal) });
    }
    return events;
  }

  if (method === "session/statusChanged") {
    return [
      {
        type: "session_lifecycle",
        action: "status_changed",
        sessionId: p.sessionId ?? p.session_id,
        status: p.status,
        flags: p.flags,
        activity: p.activity,
        activeTurnId: p.activeTurnId ?? p.active_turn_id,
      },
    ];
  }

  if (
    method === "session/created" ||
    method === "session/deleted" ||
    method === "session/archived" ||
    method === "session/closed" ||
    method === "session/cwdChanged"
  ) {
    const action = method.slice("session/".length);
    return [
      {
        type: "session_lifecycle",
        action,
        sessionId: p.sessionId ?? p.session_id ?? (p.session as { id?: string } | undefined)?.id,
        session: p.session,
        cwd: p.cwd,
        archived: p.archived,
        deletedSessionIds: p.deletedSessionIds ?? p.deleted_session_ids,
      },
    ];
  }

  if (method === "credential/changed") {
    return [
      {
        type: "session_lifecycle",
        action: "credential_changed",
        credentialId: p.credentialId ?? p.credential_id,
        provider: p.provider,
        change: p.change,
      },
    ];
  }

  // ── Model retry / failure ──
  if (method === "model/queryRetrying") {
    const error = (p.error && typeof p.error === "object" ? p.error : {}) as Record<string, unknown>;
    const attempt = Number(p.attempt ?? 0);
    const maxAttempts = Number(p.maxAttempts ?? p.max_attempts ?? 0);
    const delayMs = Number(p.nextDelayMs ?? p.next_delay_ms ?? 0);
    const phase = String(p.phase ?? "").toLowerCase();
    // Native emits Scheduled (backoff > 0) then Resumed (delay 0). A zero-delay
    // CountdownTimer ticks to -1s in InteractiveMode — clear the banner instead.
    if (delayMs <= 0 || phase === "resumed") {
      return [
        {
          type: "auto_retry_end",
          success: true,
          attempt,
        },
      ];
    }
    return [
      {
        type: "auto_retry_start",
        attempt,
        maxAttempts,
        delayMs,
        errorMessage: String(error.message ?? "Provider request retrying"),
      },
    ];
  }

  if (method === "model/queryFailed") {
    const error = (p.error && typeof p.error === "object" ? p.error : {}) as Record<string, unknown>;
    return [
      {
        type: "auto_retry_end",
        success: false,
        attempt: Number(p.attempt ?? p.maxAttempts ?? p.max_attempts ?? 1),
        finalError: String(error.message ?? "Provider request failed"),
      },
    ];
  }

  // ── Interactive / misc (parent-usable; IM may ignore unknown types) ──
  if (method === "tool_call/status_updated") {
    return projectToolStatusUpdated(p, state.tools);
  }

  if (method === "permission/decision") {
    return [
      {
        type: "session_lifecycle",
        action: "permission_decision",
        sessionId: p.sessionId ?? p.session_id,
        approvalId: p.approvalId ?? p.approval_id,
        decision: p.decision,
      },
    ];
  }

  if (method === "security/alert") {
    return [
      {
        type: "session_lifecycle",
        action: "security_alert",
        code: p.code,
        message: p.message,
      },
    ];
  }

  if (method === "item/tool/requestUserInput") {
    return [
      {
        type: "session_lifecycle",
        action: "request_user_input",
        request: p.request ?? p,
        questions: p.questions,
      },
    ];
  }

  if (method === "serverRequest/resolved") {
    return [
      {
        type: "session_lifecycle",
        action: "server_request_resolved",
        requestId: p.requestId ?? p.request_id,
        turnId: p.turnId ?? p.turn_id,
      },
    ];
  }

  if (method === "message/edit/recorded") {
    return [
      {
        type: "session_lifecycle",
        action: "message_edit_recorded",
        editId: p.editId ?? p.edit_id,
        targetMessageId: p.targetMessageId ?? p.target_message_id,
        replacementMessageId: p.replacementMessageId ?? p.replacement_message_id,
      },
    ];
  }

  if (method === "turn/superseded") {
    return [
      {
        type: "session_lifecycle",
        action: "turn_superseded",
        supersededTurnId: p.supersededTurnId ?? p.superseded_turn_id,
        replacementTurnId: p.replacementTurnId ?? p.replacement_turn_id,
        editId: p.editId ?? p.edit_id,
        reason: p.reason,
      },
    ];
  }

  if (method === "turn/recoveryUpdated") {
    return [
      {
        type: "session_lifecycle",
        action: "turn_recovery",
        recovery: p.recovery,
      },
    ];
  }

  if (method === "workspace/changes/updated") {
    return [
      {
        type: "workspace_changes_updated",
        sessionId: p.sessionId ?? p.session_id,
        turnId: p.turnId ?? p.turn_id,
        scope: p.scope,
        stats: p.stats,
        changeSetStatus: p.changeSetStatus ?? p.change_set_status,
      },
    ];
  }

  if (
    method === "workspace/restoreStarted" ||
    method === "workspace/restoreCompleted"
  ) {
    return [
      {
        type: "session_lifecycle",
        action: method.replace(/\//g, "_"),
        ...p,
      },
    ];
  }

  if (method === "search/updated" || method === "search/completed" || method === "search/failed") {
    return [
      {
        type: "session_lifecycle",
        action: method.replace(/\//g, "_"),
        snapshot: p,
      },
    ];
  }

  if (method === "initialized") {
    return [
      {
        type: "session_lifecycle",
        action: "initialized",
        connectionId: p.connectionId ?? p.connection_id,
        protocolVersion: p.protocolVersion ?? p.protocol_version,
        serverInstanceId: p.serverInstanceId ?? p.server_instance_id,
      },
    ];
  }

  if (method === "runtime/warning") {
    return [
      {
        type: "session_lifecycle",
        action: "runtime_warning",
        code: p.code,
        message: p.message,
        retryable: p.retryable,
      },
    ];
  }

  if (method === "runtime/shutdown") {
    return [
      {
        type: "session_lifecycle",
        action: "runtime_shutdown",
        reason: p.reason,
      },
    ];
  }

  // Unknown Native notify — still surface so callers can observe it.
  return [
    {
      type: "session_lifecycle",
      action: "unmapped_notification",
      method,
      params: p,
    },
  ];
}
