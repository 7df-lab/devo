import type { AgentConnectionShellTask } from "../../lib/coding-agent/src/modes/agent-connection/types.js";

/** Only authoritative shell items for the requested session belong in its dock. */
export function nativeShellTasks(result: unknown, sessionId: string): AgentConnectionShellTask[] {
  const tasks = (result as { tasks?: unknown[] } | null)?.tasks;
  if (!Array.isArray(tasks)) throw new Error("Invalid task/list response");
  return tasks.flatMap((value) => {
    if (!value || typeof value !== "object") return [];
    const item = value as Record<string, unknown>;
    const payload = item.item as Record<string, unknown> | undefined;
    if (!payload || item.sessionId !== sessionId || payload.type !== "backgroundTask" || payload.taskKind !== "shell") return [];
    const state = payload.state;
    if (typeof item.id !== "string" || !["running", "completed", "failed", "cancelled", "lost"].includes(String(state))) return [];
    return [{
      id: item.id,
      command: typeof payload.command === "string" ? payload.command : "Shell",
      state: state as AgentConnectionShellTask["state"],
      ...(typeof payload.cwd === "string" ? { cwd: payload.cwd } : {}),
      ...(typeof payload.exitCode === "number" ? { exitCode: payload.exitCode } : {}),
      ...(typeof payload.output === "string" ? { output: payload.output } : {}),
    }];
  });
}
