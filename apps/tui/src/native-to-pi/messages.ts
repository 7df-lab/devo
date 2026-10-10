/**
 * Project Native Item envelopes → InteractiveMode AgentMessage[] for restore/snapshot.
 *
 * Live streaming uses events; this module must emit equivalent
 * toolCall + toolResult pairs so InteractiveMode rebuilds tool rows on resume.
 */

import { IPYTHON_STATE_RESTORED_CUSTOM_TYPE } from "@earendil-works/pi-coding-agent/messages";
import {
  bashResultFromCommandExecution,
  displayToolName,
  editResultFromFileChange,
  normalizeToolArgs,
  planResultFromItem,
  shapeToolResultOutput,
} from "./tools.js";

export type AgentMessageLike = Record<string, unknown>;

const IPYTHON_KERNEL_STATE_RESTORED_CODE = "ipythonKernelStateRestored";
const IPYTHON_KERNEL_STATE_FRESH_CODE = "ipythonKernelStateFresh";

export function pythonKernelStateMessageFromNativeItem(
  item: Record<string, unknown>,
  timestamp = Date.now(),
): AgentMessageLike | null {
  const code = item.code;
  if (code !== IPYTHON_KERNEL_STATE_RESTORED_CODE && code !== IPYTHON_KERNEL_STATE_FRESH_CODE) {
    return null;
  }
  return {
    role: "custom",
    customType: IPYTHON_STATE_RESTORED_CUSTOM_TYPE,
    content: "",
    display: true,
    details: { restored: code === IPYTHON_KERNEL_STATE_RESTORED_CODE },
    timestamp,
  };
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

function envelopeItem(envelope: unknown): Record<string, unknown> | null {
  if (!envelope || typeof envelope !== "object") return null;
  const e = envelope as Record<string, unknown>;
  const item = (e.item ?? e) as Record<string, unknown>;
  if (!item || typeof item !== "object") return null;
  return item;
}

function createdAtMs(envelope: unknown): number {
  if (!envelope || typeof envelope !== "object") return Date.now();
  const e = envelope as Record<string, unknown>;
  const raw = e.createdAt ?? e.created_at;
  if (typeof raw === "string") {
    const t = Date.parse(raw);
    if (!Number.isNaN(t)) return t;
  }
  if (typeof raw === "number") return raw;
  return Date.now();
}

function assistantToolCallMessage(
  callId: string,
  toolName: string,
  args: Record<string, unknown>,
  ts: number,
): AgentMessageLike {
  return {
    role: "assistant",
    content: [
      {
        type: "toolCall",
        id: callId,
        name: toolName,
        arguments: args,
      },
    ],
    timestamp: ts,
    stopReason: "toolUse",
    errorMessage: null,
    usage: emptyUsage(),
  };
}

function toolResultMessage(
  callId: string,
  toolName: string,
  shaped: { content: unknown; details?: unknown },
  isError: boolean,
  ts: number,
): AgentMessageLike {
  return {
    role: "toolResult",
    toolCallId: callId,
    toolName,
    content: shaped.content,
    details: shaped.details,
    isError,
    timestamp: ts,
  };
}

/** Project one Native ItemEnvelope → InteractiveMode AgentMessage[]. */
export function nativeItemToAgentMessages(envelope: unknown): AgentMessageLike[] {
  const item = envelopeItem(envelope);
  if (!item) return [];
  const type = item.type;
  const ts = createdAtMs(envelope);

  if (type === "userMessage") {
    const parts = Array.isArray(item.content) ? item.content : [];
    const content: Array<Record<string, unknown>> = [];
    for (const c of parts) {
      if (!c || typeof c !== "object") continue;
      const part = c as Record<string, unknown>;
      if (
        part.type === "text" ||
        (part.text != null && part.type !== "localImage" && part.type !== "image")
      ) {
        const text = String(part.text ?? "");
        if (text) content.push({ type: "text", text });
        continue;
      }
      if (part.type === "localImage" || part.type === "image") {
        const label =
          part.path != null
            ? `[image:${String(part.path)}]`
            : part.uri != null
              ? "[image]"
              : "[image]";
        content.push({ type: "text", text: label });
      }
    }
    if (content.length === 0) content.push({ type: "text", text: "" });
    return [{ role: "user", content, timestamp: ts }];
  }

  if (type === "assistantMessage") {
    return [
      {
        role: "assistant",
        content: [{ type: "text", text: String(item.text ?? "") }],
        timestamp: ts,
        stopReason: null,
        errorMessage: null,
        usage: emptyUsage(),
      },
    ];
  }

  if (type === "reasoning") {
    const thinking = String(item.text ?? "");
    if (!thinking) return [];
    return [
      {
        role: "assistant",
        content: [{ type: "thinking", thinking }],
        timestamp: ts,
        stopReason: null,
        errorMessage: null,
        usage: emptyUsage(),
      },
    ];
  }

  if (type === "toolCall" || type === "tool_call") {
    const callId = String(item.callId ?? item.call_id ?? item.id ?? "");
    if (!callId) return [];
    const rawName = String(item.toolName ?? item.tool_name ?? item.name ?? "tool");
    const toolName = displayToolName(rawName);
    const args = normalizeToolArgs(rawName, item.arguments ?? item.input ?? {});
    return [assistantToolCallMessage(callId, toolName, args, ts)];
  }

  // Provider-hosted tools (web_search / web_fetch / …) — same tool row as builtins.
  if (type === "hostedToolCall" || type === "hosted_tool_call") {
    const callId = String(item.callId ?? item.call_id ?? item.id ?? "");
    if (!callId) return [];
    const rawName = String(item.toolName ?? item.tool_name ?? item.name ?? "web_search");
    const toolName = displayToolName(rawName);
    const args = normalizeToolArgs(rawName, item.input ?? item.arguments ?? {});
    const shaped = shapeToolResultOutput(item.output, item.displayContent ?? item.display_content);
    const hasOutput = item.output != null || item.displayContent != null || item.display_content != null;
    if (!hasOutput) {
      return [assistantToolCallMessage(callId, toolName, args, ts)];
    }
    return [
      assistantToolCallMessage(callId, toolName, args, ts),
      toolResultMessage(callId, toolName, shaped, Boolean(item.isError ?? item.is_error), ts),
    ];
  }

  if (type === "toolResult" || type === "tool_result") {
    const callId = String(item.callId ?? item.call_id ?? "");
    if (!callId) return [];
    const toolName = displayToolName(String(item.toolName ?? item.tool_name ?? item.name ?? "tool"));
    const shaped = shapeToolResultOutput(item.output, item.displayContent ?? item.display_content);
    return [
      toolResultMessage(callId, toolName, shaped, Boolean(item.isError ?? item.is_error), ts),
    ];
  }

  if (type === "fileChange") {
    const callId = String(item.callId ?? item.call_id ?? "");
    if (!callId) return [];
    const shaped = editResultFromFileChange(item);
    const path =
      shaped.details.path ??
      (Array.isArray(item.changes) && item.changes[0] && typeof item.changes[0] === "object"
        ? String((item.changes[0] as { path?: unknown }).path ?? "")
        : "");
    const args = normalizeToolArgs("edit", path ? { path } : {});
    // Completed FileChange replaces the live ToolCall item — emit both sides.
    return [
      assistantToolCallMessage(callId, "edit", args, ts),
      toolResultMessage(callId, "edit", shaped, false, ts),
    ];
  }

  if (type === "commandExecution") {
    const callId = String(item.callId ?? item.call_id ?? "");
    if (!callId) return [];
    const command = String(item.command ?? "");
    const input = item.input;
    const args = normalizeToolArgs(
      "exec_command",
      input != null && typeof input === "object"
        ? { ...(input as object), command: command || (input as { cmd?: string }).cmd }
        : { command },
    );
    const shaped = bashResultFromCommandExecution(item);
    const hasResult = item.output != null || item.exitCode != null || item.exit_code != null;
    if (!hasResult) {
      return [assistantToolCallMessage(callId, "bash", args, ts)];
    }
    return [
      assistantToolCallMessage(callId, "bash", args, ts),
      toolResultMessage(
        callId,
        "bash",
        shaped,
        Number(item.exitCode ?? item.exit_code ?? 0) !== 0,
        ts,
      ),
    ];
  }

  // Completed update_plan calls persist as Plan items. Without this case the
  // whole plan disappears on resume/getMessages — replay them as an
  // update_plan tool-call + checklist result pair like every other tool.
  if (type === "plan") {
    const env = (envelope ?? {}) as Record<string, unknown>;
    const callId = String(item.callId ?? item.call_id ?? env.id ?? item.id ?? "");
    if (!callId) return [];
    const entries = Array.isArray(item.entries) ? item.entries : [];
    const args = { plan: entries };
    if (entries.length === 0) {
      // Started-only plan (interrupted mid-call): keep the row, no result —
      // the pairing pass below appends an "Interrupted" toolResult.
      return [assistantToolCallMessage(callId, "update_plan", args, ts)];
    }
    return [
      assistantToolCallMessage(callId, "update_plan", args, ts),
      toolResultMessage(callId, "update_plan", planResultFromItem(item), false, ts),
    ];
  }

  // RLM namespace state is a durable, informational notice rendered by the
  // existing injected-message component, not terminal warning chrome.
  if (type === "warning") {
    const kernelStateMessage = pythonKernelStateMessageFromNativeItem(item, ts);
    if (kernelStateMessage) return [kernelStateMessage];

    // Terminal turn failures persist as Warning (and may also be synthesized from
    // session/turns/list). Live path paints via turn/completed errorMessage.
    // Keep body empty so AssistantMessageComponent's error chrome is the single paint.
    const message = String(item.message ?? "").trim();
    if (!message) return [];
    return [
      {
        role: "assistant",
        content: [{ type: "text", text: "" }],
        timestamp: ts,
        stopReason: "error",
        errorMessage: message,
        usage: emptyUsage(),
      },
    ];
  }

  // User `!` bash tasks (BackgroundTask / shell) — resume must rebuild the
  // BashExecutionComponent chrome, not a tool row.
  if (type === "backgroundTask" || type === "background_task") {
    const taskKind = String(item.taskKind ?? item.task_kind ?? "").toLowerCase();
    if (taskKind && taskKind !== "shell") return [];
    const command = String(item.command ?? "").trim();
    if (!command) return [];
    const output = String(item.output ?? item.outputTail ?? item.output_tail ?? "");
    const exitCodeRaw = item.exitCode ?? item.exit_code;
    const exitCode =
      exitCodeRaw == null || exitCodeRaw === ""
        ? undefined
        : Number(exitCodeRaw);
    return [
      {
        role: "bashExecution",
        command,
        output,
        exitCode: Number.isFinite(exitCode as number) ? (exitCode as number) : undefined,
        cancelled: false,
        truncated: false,
        timestamp: ts,
        excludeFromContext: true,
      },
    ];
  }

  return [];
}

/**
 * Project a list of Native items. Pairs orphan toolResults with a synthetic
 * toolCall when the matching call was lost (e.g. FileChange-only histories are
 * already paired above). Unpaired toolCalls (interrupted mid-flight) get an
 * aborted toolResult so resume does not leave ◇ diamonds open.
 */
export function nativeItemsToAgentMessages(items: unknown): AgentMessageLike[] {
  if (!Array.isArray(items)) return [];
  const out: AgentMessageLike[] = [];
  const seenCalls = new Set<string>();
  const callsWithResults = new Set<string>();
  const callMeta = new Map<
    string,
    { toolName: string; args: Record<string, unknown>; ts: number }
  >();

  for (const envelope of items) {
    for (const msg of nativeItemToAgentMessages(envelope)) {
      if (msg.role === "assistant" && Array.isArray(msg.content)) {
        for (const part of msg.content as Array<{
          type?: string;
          id?: string;
          name?: string;
          arguments?: Record<string, unknown>;
        }>) {
          if (part.type === "toolCall" && part.id) {
            seenCalls.add(part.id);
            callMeta.set(part.id, {
              toolName: String(part.name ?? "tool"),
              args: (part.arguments ?? {}) as Record<string, unknown>,
              ts: Number(msg.timestamp ?? Date.now()),
            });
          }
        }
      }
      if (msg.role === "toolResult") {
        const callId = String(msg.toolCallId ?? "");
        if (callId) callsWithResults.add(callId);
        if (callId && !seenCalls.has(callId)) {
          // Orphan result (e.g. toolCall pruned) — synthesize the call so IM keeps the row.
          out.push(
            assistantToolCallMessage(
              callId,
              String(msg.toolName ?? "tool"),
              {},
              Number(msg.timestamp ?? Date.now()),
            ),
          );
          seenCalls.add(callId);
        }
      }
      out.push(msg);
    }
  }

  for (const callId of seenCalls) {
    if (callsWithResults.has(callId)) continue;
    const meta = callMeta.get(callId) ?? {
      toolName: "tool",
      args: {},
      ts: Date.now(),
    };
    out.push(
      toolResultMessage(
        callId,
        meta.toolName,
        {
          content: [{ type: "text", text: "Interrupted" }],
          details: { status: "aborted" },
        },
        /*isError*/ true,
        meta.ts,
      ),
    );
  }
  return out;
}

/**
 * Attach durable turn failures (from session/turns/list) onto the restored
 * message list when Warning items were not present in items/list.
 */
export function applyFailedTurnsToAgentMessages(
  messages: AgentMessageLike[],
  turns: unknown,
): AgentMessageLike[] {
  if (!Array.isArray(turns) || turns.length === 0) return messages;
  const failures: Array<{ message: string; completedAt?: number }> = [];
  for (const turn of turns) {
    if (!turn || typeof turn !== "object") continue;
    const t = turn as Record<string, unknown>;
    const status = String(t.status ?? "").toLowerCase();
    if (status !== "failed") continue;
    const err = t.error as { message?: string } | string | null | undefined;
    const message =
      typeof err === "string"
        ? err
        : String((err as { message?: string } | undefined)?.message ?? "").trim();
    if (!message) continue;
    const completedRaw = t.completedAt ?? t.completed_at;
    let completedAt: number | undefined;
    if (typeof completedRaw === "string") {
      const parsed = Date.parse(completedRaw);
      if (!Number.isNaN(parsed)) completedAt = parsed;
    } else if (typeof completedRaw === "number") {
      completedAt = completedRaw;
    }
    failures.push({ message, completedAt });
  }
  if (failures.length === 0) return messages;

  const already = new Set(
    messages
      .filter(
        (m) =>
          m.role === "assistant" &&
          (m.stopReason === "error" || typeof m.errorMessage === "string") &&
          String(m.errorMessage ?? "").trim().length > 0,
      )
      .map((m) => String(m.errorMessage).trim()),
  );

  const out = [...messages];
  for (const failure of failures) {
    if (already.has(failure.message)) continue;
    // Prefer annotating the last assistant text message when it has no error yet.
    let annotated = false;
    for (let i = out.length - 1; i >= 0; i--) {
      const msg = out[i];
      if (msg?.role !== "assistant") continue;
      if (msg.stopReason === "toolUse") continue;
      if (msg.stopReason === "error" && msg.errorMessage) continue;
      out[i] = {
        ...msg,
        stopReason: "error",
        errorMessage: failure.message,
      };
      annotated = true;
      break;
    }
    if (!annotated) {
      out.push({
        role: "assistant",
        content: [{ type: "text", text: "" }],
        timestamp: failure.completedAt ?? Date.now(),
        stopReason: "error",
        errorMessage: failure.message,
        usage: emptyUsage(),
      });
    }
    already.add(failure.message);
  }
  return out;
}

/** Map Native inputModalities → Model.input modality list. */
export function modalitiesToInput(modalities: unknown): string[] {
  if (!Array.isArray(modalities) || modalities.length === 0) return ["text"];
  const out: string[] = [];
  for (const m of modalities) {
    const raw = typeof m === "string" ? m : (m as { type?: string })?.type ?? String(m);
    const v = String(raw).toLowerCase();
    if (v === "text" || v === "image" || v === "audio") out.push(v);
  }
  return out.length > 0 ? out : ["text"];
}

export function occupancyToContextUsage(occupancy: unknown):
  | { tokens: number; contextWindow: number; percent: number }
  | undefined {
  if (!occupancy || typeof occupancy !== "object") return undefined;
  const o = occupancy as Record<string, unknown>;
  const tokens = Number(
    o.totalTokens ??
      o.total_tokens ??
      o.tokensUsed ??
      o.tokens_used ??
      o.tokens ??
      0,
  );
  const contextWindow = Number(
    o.contextWindowTokens ??
      o.context_window_tokens ??
      o.contextWindow ??
      o.context_window ??
      o.maxTokens ??
      o.max_tokens ??
      0,
  );
  if (!contextWindow && !tokens) return undefined;
  const percent =
    typeof o.percent === "number"
      ? o.percent
      : contextWindow > 0
        ? Math.min(100, Math.round((tokens / contextWindow) * 1000) / 10)
        : 0;
  return { tokens, contextWindow: contextWindow || 1, percent };
}

export function emptyGoalState() {
  return {
    active: false,
    status: "idle" as const,
    tokensUsed: 0,
    timeUsedSeconds: 0,
    continuationsUsed: 0,
  };
}

export function nativeGoalToAgentGoal(goal: unknown) {
  if (!goal || typeof goal !== "object") return emptyGoalState();
  const g = goal as Record<string, unknown>;
  const statusRaw = String(g.status ?? "idle").toLowerCase();
  const statusMap: Record<string, string> = {
    idle: "idle",
    active: "active",
    running: "active",
    paused: "paused",
    budget_limited: "budget_limited",
    budgetlimited: "budget_limited",
    usagelimited: "budget_limited",
    usage_limited: "budget_limited",
    blocked: "paused",
    complete: "complete",
    completed: "complete",
    error: "error",
    failed: "error",
    canceled: "idle",
    cancelled: "idle",
    cleared: "idle",
  };
  const status = statusMap[statusRaw] ?? "idle";
  return {
    active: status === "active",
    status,
    goalId: g.goalId ?? g.goal_id ?? g.id,
    objective: g.objective ?? g.text,
    tokenBudget: g.tokenBudget ?? g.token_budget,
    tokensUsed: Number(g.tokensUsed ?? g.tokens_used ?? 0),
    timeUsedSeconds: Number(g.timeUsedSeconds ?? g.time_used_seconds ?? 0),
    continuationsUsed: Number(g.continuationsUsed ?? g.continuations_used ?? 0),
    lastReason: g.lastReason ?? g.last_reason,
    lastError: g.lastError ?? g.last_error,
  };
}
