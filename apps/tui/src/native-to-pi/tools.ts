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
    const callId = String(item.callId ?? item.call_id ?? item.id ?? itemId);
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

function isIpythonDetailsShape(value: Record<string, unknown>): boolean {
  return (
    typeof value.status === "string" ||
    typeof value.durationMs === "number" ||
    typeof value.stdout === "string" ||
    typeof value.stderr === "string" ||
    typeof value.errorName === "string" ||
    typeof value.errorEname === "string" ||
    (value.error != null && typeof value.error === "object")
  );
}

function normalizeIpythonDetails(raw: Record<string, unknown>): Record<string, unknown> {
  const errorName =
    typeof raw.errorEname === "string"
      ? raw.errorEname
      : typeof raw.errorName === "string"
        ? raw.errorName
        : undefined;
  const errorValue =
    typeof raw.errorValue === "string"
      ? raw.errorValue
      : typeof (raw as { evalue?: unknown }).evalue === "string"
        ? String((raw as { evalue: string }).evalue)
        : "";
  let error = raw.error;
  if ((!error || typeof error !== "object") && errorName) {
    error = {
      ename: errorName,
      evalue: errorValue,
      traceback: Array.isArray(raw.traceback) ? raw.traceback : [],
    };
  }
  return {
    ...raw,
    ...(errorName ? { errorEname: errorName } : {}),
    ...(error ? { error } : {}),
  };
}

function ipythonResultText(record: Record<string, unknown>): string {
  if (typeof record.output === "string" && record.output.length > 0) {
    return record.output;
  }
  const parts = [record.stdout, record.result, record.stderr].filter(
    (value): value is string => typeof value === "string" && value.length > 0,
  );
  if (parts.length > 0) return parts.join("\n");
  if (typeof record.status === "string") return `(status: ${record.status})`;
  return "";
}

/** One-line / short preview for hosted web_search hit arrays (never JSON.stringify the dump). */
export function summarizeWebSearchOutput(output: unknown): string {
  if (Array.isArray(output)) {
    const titles = output
      .slice(0, 5)
      .map((hit) => {
        if (!hit || typeof hit !== "object") return "";
        const record = hit as Record<string, unknown>;
        return String(record.title ?? record.name ?? record.url ?? "").trim();
      })
      .filter(Boolean);
    const count = output.length;
    const noun = count === 1 ? "result" : "results";
    if (titles.length === 0) return `${count} search ${noun}`;
    const more = count > titles.length ? ` (+${count - titles.length} more)` : "";
    return `${count} ${noun}: ${titles.join("; ")}${more}`;
  }
  if (output && typeof output === "object") {
    const record = output as Record<string, unknown>;
    if (Array.isArray(record.results)) return summarizeWebSearchOutput(record.results);
    if (Array.isArray(record.hits)) return summarizeWebSearchOutput(record.hits);
    if (typeof record.output === "string" && record.output.trim()) {
      return truncateDisplayText(record.output, 400);
    }
    if (typeof record.text === "string" && record.text.trim()) {
      return truncateDisplayText(record.text, 400);
    }
  }
  if (typeof output === "string") return truncateDisplayText(output, 400);
  return "";
}

function truncateDisplayText(text: string, maxChars: number): string {
  const trimmed = text.replace(/\s+/g, " ").trim();
  if (trimmed.length <= maxChars) return trimmed;
  return `${trimmed.slice(0, Math.max(0, maxChars - 1)).trimEnd()}…`;
}

/**
 * Kernel shell/pty results arrive as the model-visible envelope dict — the
 * human-readable output lives in its `output` string and every other key is
 * execution metadata (exit, duration_ms, …).
 */
export function isShellResultEnvelope(record: Record<string, unknown>): boolean {
  return (
    typeof record.command === "string" &&
    (typeof record.exit === "number" || typeof record.exitCode === "number") &&
    (typeof record.output === "string" || record.output == null)
  );
}

/** Decode a kernel shell envelope into bash-renderer content + metadata details. */
export function shellResultFromEnvelope(record: Record<string, unknown>): {
  content: Array<{ type: string; text: string }>;
  details: Record<string, unknown>;
} {
  const output = typeof record.output === "string" ? record.output : "";
  const exit = typeof record.exit === "number" ? record.exit : record.exitCode;
  return {
    content: [{ type: "text", text: output || "(no output)" }],
    details: {
      exitCode: typeof exit === "number" ? exit : undefined,
      durationMs: typeof record.duration_ms === "number" ? record.duration_ms : undefined,
      command: record.command,
    },
  };
}

function displayContentText(displayContent: unknown): string {
  if (typeof displayContent === "string") return displayContent;
  if (Array.isArray(displayContent)) {
    return displayContent
      .map((part) =>
        part && typeof part === "object" && typeof (part as { text?: unknown }).text === "string"
          ? String((part as { text: string }).text)
          : "",
      )
      .filter(Boolean)
      .join("\n");
  }
  return "";
}

/** Normalize Native tool result output into ToolResultMessage fields. */
export function shapeToolResultOutput(
  output: unknown,
  displayContent?: unknown,
): { content: Array<{ type: string; text: string }>; details?: Record<string, unknown> } {
  const withContent = (value: Record<string, unknown>, fallbackText = "") => {
    if (Array.isArray(value.content)) {
      return value as { content: Array<{ type: string; text: string }>; details?: Record<string, unknown> };
    }
    if (typeof value.content === "string") {
      return { ...value, content: [{ type: "text", text: value.content }] };
    }
    const text =
      typeof value.text === "string"
        ? value.text
        : typeof value.message === "string"
          ? value.message
          : fallbackText || truncateDisplayText(JSON.stringify(value), 800);
    return { ...value, content: [{ type: "text", text }] };
  };

  // Hosted web_search often ships a raw hit array as `output`. Never stringify it
  // into the tool row — keep a short summary in content and the raw hits in details.
  if (Array.isArray(output)) {
    const summary =
      summarizeWebSearchOutput(output) ||
      displayContentText(displayContent) ||
      `${output.length} results`;
    return {
      content: [{ type: "text", text: summary }],
      details: { resultKind: "web_search", hits: output, hitCount: output.length },
    };
  }

  if (output != null && typeof output === "object") {
    const record = output as Record<string, unknown>;
    if (record.details != null && typeof record.details === "object") {
      const details = normalizeIpythonDetails(record.details as Record<string, unknown>);
      return withContent({ ...record, details });
    }
    if (isIpythonDetailsShape(record) && !Array.isArray(record.content)) {
      return {
        content: [{ type: "text", text: ipythonResultText(record) }],
        details: normalizeIpythonDetails(record),
      };
    }
    // Kernel shell results: decode the envelope instead of stringifying it
    // into the tool row (the raw JSON dump is never what a reader wants).
    if (isShellResultEnvelope(record)) {
      return shellResultFromEnvelope(record);
    }
    if (Array.isArray(record.results) || Array.isArray(record.hits)) {
      const hits = (Array.isArray(record.results) ? record.results : record.hits) as unknown[];
      const summary =
        summarizeWebSearchOutput(hits) ||
        displayContentText(displayContent) ||
        `${hits.length} results`;
      return {
        content: [{ type: "text", text: summary }],
        details: { ...record, resultKind: "web_search", hits, hitCount: hits.length },
      };
    }
    return withContent(record, displayContentText(displayContent));
  }
  if (typeof output === "string") {
    const display = displayContentText(displayContent);
    // Prefer a short display_content when the raw string is a huge dump.
    if (display && output.length > 800 && display.length < output.length) {
      return {
        content: [{ type: "text", text: display }],
        details: { resultKind: "truncated_output", fullText: output },
      };
    }
    // Hosted / skill web_search sometimes arrives as a JSON-encoded hit array string.
    const trimmed = output.trim();
    if (trimmed.startsWith("[") || trimmed.startsWith("{")) {
      try {
        const parsed = JSON.parse(trimmed) as unknown;
        if (
          parsed != null &&
          typeof parsed === "object" &&
          !Array.isArray(parsed) &&
          isShellResultEnvelope(parsed as Record<string, unknown>)
        ) {
          return shellResultFromEnvelope(parsed as Record<string, unknown>);
        }
        const summary = summarizeWebSearchOutput(parsed);
        if (summary) {
          return {
            content: [{ type: "text", text: summary }],
            details: { resultKind: "web_search", rawText: output },
          };
        }
      } catch {
        // not JSON — fall through
      }
    }
    if (output.length > 2_000) {
      return {
        content: [{ type: "text", text: truncateDisplayText(output, 400) }],
        details: { resultKind: "truncated_output", fullText: output },
      };
    }
    return { content: [{ type: "text", text: output }] };
  }
  if (displayContent != null) {
    if (Array.isArray(displayContent)) {
      return { content: displayContent as Array<{ type: string; text: string }> };
    }
    if (typeof displayContent === "string") {
      return { content: [{ type: "text", text: displayContent }] };
    }
    if (typeof displayContent === "object") {
      return withContent(displayContent as Record<string, unknown>);
    }
  }
  return { content: [{ type: "text", text: "" }] };
}
