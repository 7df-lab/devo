import { stripVTControlCharacters } from "node:util";

const HOST_LABELS = new Map([
  ["agent", "Agent message"],
  ["backgroundTask", "Background notification"],
  ["schedule", "Scheduled notification"],
]);

export function hostMessageLabel(origin: Record<string, unknown>): string {
  return HOST_LABELS.get(String(origin.source)) ?? "Host notification";
}

export function cleanHostDisplayText(text: string): string {
  return stripVTControlCharacters(text).replace(/\r\n?/g, "\n").replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, "");
}

export function hostMessageSender(origin: Record<string, unknown>): string | undefined {
  const named = origin.senderName ?? origin.sender_name;
  const sender = typeof named === "string" && named.trim() ? named
    // For automated messages, sender is an opaque session/work ID, not a name.
    : origin.source === "backgroundTask" || origin.source === "schedule" ? undefined
    : origin.sender ?? "Host";
  return typeof sender === "string" ? cleanHostDisplayText(sender).replace(/\s+/g, " ").trim() || undefined : undefined;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

/** Interpret known terminal shell results only for host-authored background messages.
 * Leave persisted text, provenance and the model's input unchanged. */
export function backgroundShellDisplay(origin: Record<string, unknown>, text: string): { summary: string; output: string } | undefined {
  if (origin.source !== "backgroundTask" || !text.trimStart().startsWith("{")) return undefined;
  let payload: Record<string, unknown> | undefined;
  try {
    payload = record(JSON.parse(text));
  } catch {
    return undefined;
  }
  const result = record(payload?.result);
  const task = record(payload?.task);
  const taskCommand = record(task?.command);
  let command: string;
  let exit: unknown;
  let state: string;
  let output: unknown;
  let error: unknown;
  if (payload?.status === "ok" && result?.running === false && typeof result.handle_id === "string" && typeof result.command === "string") {
    // Python bash() results are snapshots of the managed shell handle.
    command = result.command;
    exit = result.exit_code;
    error = result.error;
    state = typeof error === "string" && error.trim() ? "failed" : "finished";
    output = result.output;
  } else if (payload?.outcome === "terminal" && task?.kind === "command" && typeof taskCommand?.command === "string" &&
    (task.state === "completed" || task.state === "failed" || task.state === "canceled")) {
    // Native exec background tasks already provide formatted output.
    command = taskCommand.command;
    exit = taskCommand.exit_code;
    state = task.state === "completed" ? "finished" : task.state;
    output = payload.output;
  } else {
    return undefined;
  }
  const visibleCommand = cleanHostDisplayText(command).replace(/\s+/g, " ").trim();
  const exitLabel = typeof exit === "number" && Number.isSafeInteger(exit) ? `exit ${exit}` : undefined;
  const status = state === "finished" ? exitLabel ?? state : [state, exitLabel].filter(Boolean).join(" · ");
  let visibleOutput = typeof output === "string" ? cleanHostDisplayText(output).trimEnd() : "";
  if (payload?.outcome === "terminal" && exitLabel) {
    const prefix = `Process exited with code ${exit}`;
    if (visibleOutput === prefix) visibleOutput = "";
    else if (visibleOutput.startsWith(`${prefix}\n`)) visibleOutput = visibleOutput.slice(prefix.length + 1);
  }
  const errorText = typeof error === "string" ? cleanHostDisplayText(error).trim() : "";
  return {
    summary: [status, visibleCommand].filter(Boolean).join(" · "),
    output: [visibleOutput, errorText && errorText !== visibleOutput.trim() ? errorText : ""].filter(Boolean).join("\n"),
  };
}
