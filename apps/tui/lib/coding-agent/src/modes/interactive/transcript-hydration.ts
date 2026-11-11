import type { AgentConnectionEvent, AgentConnectionSessionEvent, AgentConnectionSnapshot } from "../agent-connection/types.js";

/** Repair queued delta snapshots created before the adapter learned an item's birth prefix. */
export function hydrateQueuedTextEvent(
  event: Extract<AgentConnectionEvent, { type: "session_event" }>,
  prefixes: NonNullable<AgentConnectionSnapshot["streamingPrefixes"]>,
): AgentConnectionSessionEvent {
  const missing = event.sourceItem?.missingTextBaseIds ?? [];
  const sessionEvent = event.event;
  if (!missing.length || (sessionEvent.type !== "message_start" && sessionEvent.type !== "message_update" && sessionEvent.type !== "message_end")) return sessionEvent;
  const message = sessionEvent.message;
  if (message.role !== "assistant") return sessionEvent;
  let content = message.content;
  for (const id of missing) {
    const prefix = prefixes[id];
    if (!prefix?.prefix) continue;
    let found = false;
    content = content.map(part => {
      if (prefix.kind === "text" && part.type === "text") { found = true; return { ...part, text: prefix.prefix + part.text }; }
      if (prefix.kind === "thinking" && part.type === "thinking") { found = true; return { ...part, thinking: prefix.prefix + part.thinking }; }
      return part;
    });
    if (!found) {
      if (prefix.kind === "text") content.push({ type: "text", text: prefix.prefix });
      else content.unshift({ type: "thinking", thinking: prefix.prefix });
    }
  }
  return { ...sessionEvent, message: { ...message, content } };
}
