/**
 * Native tool-item projection → InteractiveMode tool lifecycle events.
 *
 * Lifecycle (matches agent-loop / InteractiveMode):
 *   tool_pending → tool_args_update* → tool_args_complete → tool_execution_start
 *     → tool_execution_update* (execution progress) → tool_execution_end
 *
 * Native `item/toolCall/inputDelta` is argument streaming, NOT result streaming.
 * Native `item/commandExecution/outputDelta` / ToolProgress is result streaming.
 */

import { parseStreamingJson } from "@earendil-works/pi-ai";

export type NativeToPiToolState = {
  toolNameByCall: Map<string, string>;
  /** Native item id → tool call id (deltas only carry itemId). */
  callIdByItemId: Map<string, string>;
  /** Accumulated provider partial_json per call. */
  partialJsonByCall: Map<string, string>;
  /** Calls that already received tool_pending. */
  pendingCalls: Set<string>;
  /** Calls that already received tool_execution_start. */
  startedCalls: Set<string>;
  /** Calls whose args were marked complete. */
  argsCompleteCalls: Set<string>;
  /** Calls that already emitted tool_execution_end (ignore late duplicates). */
  finishedCalls: Set<string>;
};

export function createNativeToPiToolState(): NativeToPiToolState {
  return {
    toolNameByCall: new Map(),
    callIdByItemId: new Map(),
    partialJsonByCall: new Map(),
    pendingCalls: new Set(),
    startedCalls: new Set(),
    argsCompleteCalls: new Set(),
    finishedCalls: new Set(),
  };
}

/** Map Devo / registry tool names onto built-in renderers when possible. */
export function displayToolName(rawName: string): string {
  const name = rawName.trim();
  switch (name) {
    case "exec_command":
    case "shell_command":
    case "write_stdin":
    case "command":
      return "bash";
    case "apply_patch":
    case "file_change":
    case "fileChange":
      return "edit";
    default:
      return name;
  }
}

function isEmptyArgs(args: unknown): boolean {
  if (args == null) return true;
  if (typeof args !== "object") return false;
  return Object.keys(args as object).length === 0;
}

/** Normalize Native tool input into renderer-friendly args. */
export function normalizeToolArgs(toolName: string, rawArgs: unknown): Record<string, unknown> {
  const display = displayToolName(toolName);
  if (typeof rawArgs === "string") {
    try {
      const parsed = JSON.parse(rawArgs) as unknown;
      if (typeof parsed === "string" || (parsed != null && typeof parsed === "object")) rawArgs = parsed;
    } catch {
      // Plain inputs from hosted tools are still useful arguments below.
    }
  }
  if (typeof rawArgs === "string") {
    if (/^web[_-]?search$/i.test(display)) return { query: rawArgs };
    if (/^(web[_-]?fetch|fetch_url)$/i.test(display)) return { url: rawArgs };
  }
  const record =
    rawArgs != null && typeof rawArgs === "object" && !Array.isArray(rawArgs)
      ? (rawArgs as Record<string, unknown>)
      : {};

  if (display === "bash") {
    const command =
      typeof record.command === "string"
        ? record.command
        : typeof record.cmd === "string"
          ? record.cmd
          : typeof record.chars === "string"
            ? record.chars
            : "";
    // Kernel shell tools measure `timeout` in milliseconds while the renderer's
    // bash schema uses seconds — project to seconds so the call-line label
    // ("(timeout Ns)") stays honest for kernel-sourced calls.
    const timeoutMs = typeof record.timeout === "number" ? record.timeout : undefined;
    const timeoutSeconds = timeoutMs == null ? undefined : Math.round(timeoutMs / 100) / 10;
    const normalized = command ? { ...record, command } : { ...record };
    return timeoutSeconds == null ? normalized : { ...normalized, timeout: timeoutSeconds };
  }

  if (display === "edit" || display === "write") {
    const path =
      typeof record.path === "string"
        ? record.path
        : typeof record.filePath === "string"
          ? record.filePath
          : typeof record.file_path === "string"
            ? record.file_path
            : undefined;
    return path != null ? { ...record, path } : { ...record };
  }

  return { ...record };
}

function extractPartialJsonChunk(delta: string): string {
  const trimmed = delta.trim();
  if (!trimmed) return "";
  if (trimmed.startsWith("{")) {
    try {
      const parsed = JSON.parse(trimmed) as { partial_json?: unknown; partialJson?: unknown };
      if (typeof parsed.partial_json === "string") return parsed.partial_json;
      if (typeof parsed.partialJson === "string") return parsed.partialJson;
    } catch {
      // Treat as raw partial JSON fragment.
    }
  }
  return delta;
}

/** The input-delta payload names the call: `{"tool_use_id": "...", ...}`. */
function extractPartialJsonCallId(delta: string): string {
  const trimmed = delta.trim();
  if (!trimmed.startsWith("{")) return "";
  try {
    const parsed = JSON.parse(trimmed) as { tool_use_id?: unknown; toolCallId?: unknown };
    const id = parsed.tool_use_id ?? parsed.toolCallId;
    return typeof id === "string" ? id : "";
  } catch {
    return "";
  }
}

function extractOutputDeltaText(delta: string): string {
  const trimmed = delta.trim();
  if (!trimmed) return "";
  if (trimmed.startsWith("{")) {
    try {
      const parsed = JSON.parse(trimmed) as { text?: unknown; delta?: unknown };
      if (typeof parsed.text === "string") return parsed.text;
      if (typeof parsed.delta === "string") return parsed.delta;
    } catch {
      // fall through
    }
  }
  return delta;
}

function rememberCall(
  state: NativeToPiToolState,
  callId: string,
  itemId: string | undefined,
  toolName: string,
): void {
  state.toolNameByCall.set(callId, toolName);
  if (itemId) state.callIdByItemId.set(itemId, callId);
}

function resolveCallId(
  state: NativeToPiToolState,
  params: Record<string, unknown>,
): string {
  const direct = String(params.callId ?? params.call_id ?? params.toolCallId ?? params.tool_call_id ?? "");
  if (direct) return direct;
  const itemId = String(params.itemId ?? params.item_id ?? "");
  if (itemId && state.callIdByItemId.has(itemId)) {
    return state.callIdByItemId.get(itemId)!;
  }
  return itemId;
}

function clearCall(state: NativeToPiToolState, callId: string): void {
  state.toolNameByCall.delete(callId);
  state.partialJsonByCall.delete(callId);
  state.pendingCalls.delete(callId);
  state.startedCalls.delete(callId);
  state.argsCompleteCalls.delete(callId);
  for (const [itemId, mapped] of state.callIdByItemId) {
    if (mapped === callId) state.callIdByItemId.delete(itemId);
  }
}

/** Close in-flight tools as aborted (Esc/Ctrl+C before item/completed). */
export function projectPendingToolsAborted(
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  const callIds = new Set<string>([...state.pendingCalls, ...state.startedCalls]);
  const events: Array<Record<string, unknown>> = [];
  for (const callId of callIds) {
    if (!callId || state.finishedCalls.has(callId)) continue;
    events.push(
      ...projectToolExecutionEnd(
        callId,
        {
          content: [{ type: "text", text: "Interrupted" }],
          details: { status: "aborted" },
        },
        /*isError*/ true,
        state,
      ),
    );
  }
  return events;
}

export function projectToolItemStarted(
  envelope: Record<string, unknown>,
  item: Record<string, unknown>,
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  const itemId = String(envelope.id ?? item.id ?? "");
  const type = item.type;
  const events: Array<Record<string, unknown>> = [];

  if (type === "toolCall" || type === "tool_call") {
    const callId = String(item.callId ?? item.call_id ?? item.id ?? itemId);
    if (state.finishedCalls.has(callId)) return [];
    const rawName = String(item.toolName ?? item.tool_name ?? item.name ?? "tool");
    const toolName = displayToolName(rawName);
    const args = normalizeToolArgs(rawName, item.arguments ?? item.input ?? {});
    rememberCall(state, callId, itemId || undefined, toolName);

    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName,
        args,
      });
    } else if (!isEmptyArgs(args)) {
      // Re-broadcast with assembled args after streaming.
      events.push({
        type: "tool_args_update",
        toolCallId: callId,
        toolName,
        args,
      });
      if (!state.argsCompleteCalls.has(callId)) {
        state.argsCompleteCalls.add(callId);
        events.push({ type: "tool_args_complete", toolCallId: callId });
      }
    }
    return events;
  }

  if (type === "hostedToolCall" || type === "hosted_tool_call") {
    const callId = String(item.callId || item.call_id || item.id || itemId);
    if (!callId || state.finishedCalls.has(callId)) return [];
    const rawName = String(item.toolName ?? item.tool_name ?? item.name ?? "web_search");
    const toolName = displayToolName(rawName);
    const args = normalizeToolArgs(rawName, item.input ?? item.arguments ?? {});
    rememberCall(state, callId, itemId || undefined, toolName);
    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName,
        args,
      });
    } else if (!isEmptyArgs(args)) {
      events.push({
        type: "tool_args_update",
        toolCallId: callId,
        toolName,
        args,
      });
    }
    if (!state.argsCompleteCalls.has(callId)) {
      state.argsCompleteCalls.add(callId);
      events.push({ type: "tool_args_complete", toolCallId: callId });
    }
    return events;
  }

  if (type === "commandExecution") {
    const callId = String(item.callId ?? item.call_id ?? itemId);
    const command = String(item.command ?? "");
    const input = item.input;
    const args = normalizeToolArgs(
      "exec_command",
      input != null && typeof input === "object"
        ? { ...(input as object), command: command || (input as { cmd?: string }).cmd }
        : { command },
    );
    rememberCall(state, callId, itemId || undefined, "bash");
    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName: "bash",
        args,
      });
    } else {
      events.push({
        type: "tool_args_update",
        toolCallId: callId,
        toolName: "bash",
        args,
      });
      if (!state.argsCompleteCalls.has(callId)) {
        state.argsCompleteCalls.add(callId);
        events.push({ type: "tool_args_complete", toolCallId: callId });
      }
    }
    return events;
  }

  if (type === "fileChange") {
    const callId = String(item.callId ?? item.call_id ?? itemId);
    const changes = Array.isArray(item.changes) ? item.changes : [];
    const first = changes[0] as { path?: string; kind?: string } | undefined;
    const args = normalizeToolArgs("edit", {
      path: first?.path,
      changes,
    });
    rememberCall(state, callId, itemId || undefined, "edit");
    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName: "edit",
        args,
      });
    } else {
      events.push({
        type: "tool_args_update",
        toolCallId: callId,
        toolName: "edit",
        args,
      });
    }
    return events;
  }

  if (type === "plan") {
    // update_plan tool call: the server starts an Item::Plan (empty entries)
    // instead of a ToolCall. Without this row the input-delta and
    // status_updated notifications below orphan generic "tool · queued" /
    // "tool · running" rows that nothing ever closes.
    const callId = String(item.callId ?? item.call_id ?? itemId);
    if (!callId) return events;
    rememberCall(state, callId, itemId || undefined, "update_plan");
    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName: "update_plan",
        args: {},
      });
    }
    return events;
  }

  return events;
}

export function projectToolCallInputDelta(
  params: Record<string, unknown>,
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  const itemId = String(params.itemId ?? params.item_id ?? "");
  let callId = resolveCallId(state, params);
  if (!callId) return [];

  // When no item/started mapped this item to a call id (e.g. plan items on
  // older servers), the payload's tool_use_id is the only correlation to the
  // tool_call/status_updated row — prefer it over the raw itemId fallback
  // (including over a plan start's itemId self-mapping).
  if (callId === itemId && itemId) {
    const named = extractPartialJsonCallId(String(params.delta ?? params.text ?? ""));
    if (named && named !== itemId) {
      callId = named;
      state.callIdByItemId.set(itemId, named);
    }
  }

  const chunk = extractPartialJsonChunk(String(params.delta ?? params.text ?? ""));
  const prev = state.partialJsonByCall.get(callId) ?? "";
  const accumulated = prev + chunk;
  state.partialJsonByCall.set(callId, accumulated);

  const args = normalizeToolArgs(
    state.toolNameByCall.get(callId) ?? "tool",
    parseStreamingJson(accumulated),
  );
  const toolName = state.toolNameByCall.get(callId) ?? "tool";

  const events: Array<Record<string, unknown>> = [];
  if (!state.pendingCalls.has(callId)) {
    state.pendingCalls.add(callId);
    events.push({
      type: "tool_pending",
      toolCallId: callId,
      toolName,
      args,
    });
  } else {
    events.push({
      type: "tool_args_update",
      toolCallId: callId,
      toolName,
      args,
    });
  }
  return events;
}

export function projectCommandExecutionOutputDelta(
  params: Record<string, unknown>,
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  const callId = resolveCallId(state, params);
  if (!callId) return [];
  const text = extractOutputDeltaText(String(params.delta ?? params.text ?? ""));
  if (!text) return [];
  return [
    {
      type: "tool_execution_update",
      toolCallId: callId,
      toolName: state.toolNameByCall.get(callId) ?? "bash",
      args: {},
      partialResult: { content: [{ type: "text", text }] },
    },
  ];
}

export function projectToolStatusUpdated(
  params: Record<string, unknown>,
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  const callId = resolveCallId(state, params);
  if (!callId) return [];
  const status = String(params.status ?? "").toLowerCase();
  if (status !== "in_progress" && status !== "inprogress" && status !== "running") {
    return [];
  }
  if (state.startedCalls.has(callId)) return [];

  const toolName = state.toolNameByCall.get(callId) ?? "tool";
  const events: Array<Record<string, unknown>> = [];
  if (!state.argsCompleteCalls.has(callId)) {
    state.argsCompleteCalls.add(callId);
    events.push({ type: "tool_args_complete", toolCallId: callId });
  }
  state.startedCalls.add(callId);
  events.push({
    type: "tool_execution_start",
    toolCallId: callId,
    toolName,
    args: {},
  });
  return events;
}

export function projectToolExecutionEnd(
  callId: string,
  result: unknown,
  isError: boolean,
  state: NativeToPiToolState,
): Array<Record<string, unknown>> {
  if (!callId) return [];
  if (state.finishedCalls.has(callId)) return [];
  const toolName = state.toolNameByCall.get(callId) ?? "tool";
  const events: Array<Record<string, unknown>> = [];

  // Ensure running→done transition even if status_updated was missed.
  if (!state.startedCalls.has(callId)) {
    if (!state.pendingCalls.has(callId)) {
      state.pendingCalls.add(callId);
      events.push({
        type: "tool_pending",
        toolCallId: callId,
        toolName,
        args: {},
      });
    }
    if (!state.argsCompleteCalls.has(callId)) {
      state.argsCompleteCalls.add(callId);
      events.push({ type: "tool_args_complete", toolCallId: callId });
    }
    state.startedCalls.add(callId);
    events.push({
      type: "tool_execution_start",
      toolCallId: callId,
      toolName,
      args: {},
    });
  }

  const normalizedResult = normalizeInterruptedToolResult(result, isError);

  events.push({
    type: "tool_execution_end",
    toolCallId: callId,
    toolName,
    result: normalizedResult,
    isError: normalizedResult.isError ?? isError,
  });
  state.finishedCalls.add(callId);
  clearCall(state, callId);
  return events;
}

/** Map plain interrupted tool strings onto ipython `status: aborted`. */
function normalizeInterruptedToolResult(
  result: unknown,
  isError: boolean,
): { content: unknown; details?: unknown; isError?: boolean } {
  const interruptedText = /interrupted|cancelled|canceled/i;
  if (result != null && typeof result === "object") {
    const record = result as Record<string, unknown>;
    const details =
      record.details != null && typeof record.details === "object"
        ? { ...(record.details as Record<string, unknown>) }
        : undefined;
    const text = Array.isArray(record.content)
      ? String((record.content as Array<{ text?: string }>)[0]?.text ?? "")
      : typeof record.content === "string"
        ? record.content
        : "";
    if (details?.status === "aborted") {
      return { ...record, content: record.content, details, isError: true };
    }
    if (isError && interruptedText.test(text)) {
      return {
        ...record,
        content: record.content,
        details: { ...(details ?? {}), status: "aborted" },
        isError: true,
      };
    }
    return { ...record, content: record.content };
  }
  if (typeof result === "string" && interruptedText.test(result)) {
    return {
      content: [{ type: "text", text: result }],
      details: { status: "aborted" },
      isError: true,
    };
  }
  return { content: result };
}

export function bashResultFromCommandExecution(item: Record<string, unknown>): {
  content: Array<{ type: string; text: string }>;
  details?: Record<string, unknown>;
} {
  const output = item.output == null ? "" : String(item.output);
  const exitCode = item.exitCode ?? item.exit_code;
  return {
    content: [{ type: "text", text: output }],
    details: {
      exitCode: exitCode == null ? undefined : Number(exitCode),
      output,
    },
  };
}

/** Normalize Native plan step statuses (camelCase wire, snake_case tolerated). */
export function planStatusText(status: unknown): string {
  const raw = String(status ?? "").toLowerCase();
  if (raw === "completed") return "completed";
  if (raw === "in_progress" || raw === "inprogress") return "inProgress";
  return "pending";
}

/**
 * Project a Native Plan item into an `update_plan` tool result checklist.
 * The TUI's Plan cell renders `details.plan` as a structured checklist; this
 * text remains a readable fallback for transcript consumers.
 */
export function planResultFromItem(item: Record<string, unknown>): {
  content: Array<{ type: string; text: string }>;
  details: Record<string, unknown>;
} {
  const entries = (Array.isArray(item.entries) ? item.entries : [])
    .map((entry) => {
      if (!entry || typeof entry !== "object") return null;
      const record = entry as Record<string, unknown>;
      const step = String(record.step ?? "").trim();
      if (!step) return null;
      return { step, status: planStatusText(record.status) };
    })
    .filter((entry): entry is { step: string; status: string } => entry !== null);

  // Proposed-plan markdown (plan mode / legacy TurnItem::Plan) collapses into a
  // single completed entry holding the whole blob — render it verbatim instead
  // of dressing a document up as a one-item checklist.
  if (entries.length === 1 && (entries[0].step.length > 200 || entries[0].step.includes("\n"))) {
    return {
      content: [{ type: "text", text: entries[0].step }],
      details: { plan: entries },
    };
  }

  const lines = entries.map(({ step, status }) => {
    const marker =
      status === "completed" ? "[x]" : status === "inProgress" ? "[~]" : "[ ]";
    return `${marker} ${step}`;
  });
  const text =
    lines.length > 0
      ? `Plan (${entries.length} step${entries.length === 1 ? "" : "s"}):\n${lines.join("\n")}`
      : "Plan updated";
  return {
    content: [{ type: "text", text }],
    details: { plan: entries },
  };
}

/** Map Native FileChange item → edit tool result (`details.diff`). */
export function editResultFromFileChange(item: Record<string, unknown>): {
  content: Array<{ type: string; text: string }>;
  details: { diff?: string; path?: string; changes?: unknown };
} {  const changes = Array.isArray(item.changes) ? item.changes : [];
  const diffs: string[] = [];
  let primaryPath: string | undefined;
  const texts: string[] = [];

  for (const entry of changes) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    const path = record.path != null ? String(record.path) : undefined;
    if (path && !primaryPath) primaryPath = path;
    const change = (record.change ?? record) as Record<string, unknown>;
    const kind = String(change.type ?? change.kind ?? "");
    if (kind === "update" || change.unifiedDiff != null || change.unified_diff != null) {
      const diff = String(change.unifiedDiff ?? change.unified_diff ?? "");
      if (diff) diffs.push(diff);
      texts.push(path ? `Edited ${path}` : "Edited file");
    } else if (kind === "add") {
      const content = String(change.content ?? "");
      if (content) {
        const lines = content.split("\n");
        diffs.push(
          [`--- /dev/null`, `+++ ${path ?? "file"}`, `@@ -0,0 +1,${lines.length} @@`]
            .concat(lines.map((line) => `+${line}`))
            .join("\n"),
        );
      }
      texts.push(path ? `Created ${path}` : "Created file");
    } else if (kind === "delete") {
      texts.push(path ? `Deleted ${path}` : "Deleted file");
    }
  }

  const combinedDiff = diffs.join("\n");
  return {
    content: [{ type: "text", text: texts.join("\n") || "File change" }],
    details: {
      ...(combinedDiff ? { diff: combinedDiff } : {}),
      ...(primaryPath ? { path: primaryPath } : {}),
      changes,
    },
  };
}

/** Summaries from `workspace/changes/read` views for the turn recap. */
export function fileChangeSummariesFromWorkspaceViews(
  result: unknown,
): Array<{ path: string; added: number; removed: number }> {
  if (!result || typeof result !== "object") return [];
  const views = (result as { views?: unknown }).views;
  if (!Array.isArray(views)) return [];
  const out: Array<{ path: string; added: number; removed: number }> = [];
  for (const view of views) {
    if (!view || typeof view !== "object") continue;
    const files = (view as { files?: unknown }).files;
    if (!Array.isArray(files)) continue;
    for (const file of files) {
      if (!file || typeof file !== "object") continue;
      const record = file as Record<string, unknown>;
      const path = record.path != null ? String(record.path) : "";
      if (!path) continue;
      const added = Number(record.additions ?? 0);
      const removed = Number(record.deletions ?? 0);
      if (added === 0 && removed === 0) {
        // Still surface renamed/added/deleted with unknown counts as 1/0 or 0/1.
        const status = String(record.status ?? "").toLowerCase();
        if (status === "added" || status === "untracked") out.push({ path, added: 1, removed: 0 });
        else if (status === "deleted") out.push({ path, added: 0, removed: 1 });
        else out.push({ path, added: 1, removed: 1 });
      } else {
        out.push({ path, added, removed });
      }
    }
  }
  return out;
}

export { shapeToolResultOutput, summarizeWebSearchOutput, isShellResultEnvelope, shellResultFromEnvelope } from "./tool-results.js";
