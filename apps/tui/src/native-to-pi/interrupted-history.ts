import { nativeHistoryMessageTurnId, setNativeHistoryMessageTurnId, type AgentMessageLike } from "./messages.js";

const restoredAborts = new WeakMap<object, string>();

/** Restore abort chrome from durable turn state, not transient local Esc events. */
export function applyInterruptedTurnsToAgentMessages(
  messages: AgentMessageLike[],
  turns: unknown,
): AgentMessageLike[] {
  if (!Array.isArray(turns)) return messages;
  const ordered = turns.filter((turn): turn is Record<string, unknown> =>
    !!turn && typeof turn === "object" && typeof turn.id === "string",
  ).sort((a, b) => Number(a.sequence ?? 0) - Number(b.sequence ?? 0));
  const additions = new Map<number, AgentMessageLike[]>();
  const seen = new Set(messages.flatMap(message => {
    const id = restoredAborts.get(message);
    return id ? [id] : [];
  }));
  for (let t = 0; t < ordered.length; t++) {
    const turn = ordered[t];
    const id = String(turn.id);
    if (turn.status !== "interrupted" || seen.has(id)) continue;
    seen.add(id);
    const owned = messages.flatMap((message, index) => nativeHistoryMessageTurnId(message) === id ? [index] : []);
    // No item can exist when interrupted before admission produced any output.
    // Place that terminal marker before the next known turn, never on its reply.
    let at = owned.length ? owned[owned.length - 1] + 1 : messages.length;
    if (!owned.length) {
      const later = new Set(ordered.slice(t + 1).map(value => String(value.id)));
      const next = messages.findIndex(message => later.has(nativeHistoryMessageTurnId(message) ?? ""));
      if (next >= 0) at = next;
    }
    const completed = Date.parse(String(turn.completedAt ?? turn.completed_at ?? turn.startedAt ?? ""));
    const marker: AgentMessageLike = {
      role: "assistant",
      content: [{ type: "text", text: "" }],
      timestamp: Number.isNaN(completed) ? 0 : completed,
      stopReason: "aborted",
      errorMessage: null,
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 },
    };
    restoredAborts.set(marker, id);
    setNativeHistoryMessageTurnId(marker, id);
    const bucket = additions.get(at) ?? [];
    bucket.push(marker);
    additions.set(at, bucket);
  }
  if (!additions.size) return messages;
  return messages.flatMap((message, index) => [...(additions.get(index) ?? []), message])
    .concat(additions.get(messages.length) ?? []);
}
