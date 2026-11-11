/** Native ownership lives on the envelope, never on an agent-message sender. */
export function nativeNotificationSessionId(params: unknown): string {
  if (!params || typeof params !== "object") return "";
  const p = params as Record<string, unknown>;
  const item = p.item as Record<string, unknown> | undefined;
  const session = p.session as Record<string, unknown> | undefined;
  const turn = p.turn as Record<string, unknown> | undefined;
  const goal = p.goal as Record<string, unknown> | undefined;
  const request = p.request as Record<string, unknown> | undefined;
  return String(
    p.sessionId ?? p.session_id ?? item?.sessionId ?? item?.session_id ??
    session?.id ?? turn?.sessionId ?? turn?.session_id ??
    goal?.sessionId ?? goal?.session_id ?? request?.sessionId ?? request?.session_id ?? "",
  );
}
