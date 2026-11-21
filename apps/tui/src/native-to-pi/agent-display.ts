/** Strip only the host-generated child completion preamble in the UI projection.
 * Persisted content and the model's provenance/authority boundary stay untouched.
 */
const CHILD_COMPLETION_STATUSES = new Set(["waiting_for_input", "completed", "failed", "interrupted", "canceled", "closed"]);

/** Identify host-generated child completion events without matching ordinary messages by text. */
export function childCompletionStatus(origin?: Record<string, unknown>): string | undefined {
  if (origin?.source !== "agent" || typeof origin.sender !== "string" || typeof origin.eventId !== "string") return undefined;
  const match = /^child:([^:]+):([^:]+):([^:]+)$/.exec(origin.eventId);
  if (!match || match[1] !== origin.sender || !CHILD_COMPLETION_STATUSES.has(match[3])) return undefined;
  return match[3];
}

export function childCompletionBody(origin: Record<string, unknown>, status: string, text: string): string {
  const newline = text.indexOf("\n");
  // The short preview may be only part of the header; never display it.
  if (newline < 0) return "";
  const header = text.slice(0, newline);
  if (!header.startsWith("[system] Child agent ") || !header.endsWith(` (${origin.sender}) ${status}.`)) return "";
  return text.slice(newline + 1).trim();
}
