/** Strip only the host-generated child completion preamble in the UI projection.
 * Persisted content and the model's provenance/authority boundary stay untouched.
 */
export function agentCompletionDisplay(
  origin: Record<string, unknown>,
  text: string,
): { text: string; status?: string } {
  if (origin.source !== "agent" || typeof origin.sender !== "string" || typeof origin.eventId !== "string") return { text };
  const prefix = `child:${origin.sender}:`;
  if (!origin.eventId.startsWith(prefix)) return { text };
  const identity = origin.eventId.slice(prefix.length);
  const separator = identity.lastIndexOf(":");
  if (separator <= 0) return { text };
  const status = identity.slice(separator + 1);
  if (!["waiting_for_input", "completed", "failed", "interrupted", "canceled", "closed"].includes(status)) return { text };
  const newline = text.indexOf("\n");
  const header = newline < 0 ? text : text.slice(0, newline);
  const suffix = ` (${origin.sender}) ${status}.`;
  if (!header.startsWith("[system] Child agent ") || !header.endsWith(suffix) || header.length <= "[system] Child agent ".length + suffix.length) return { text };
  return { text: newline < 0 ? "" : text.slice(newline + 1), status };
}
