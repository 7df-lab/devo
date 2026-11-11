import { textHydrationState } from "./native-to-pi/stream-hydration-state.js";
import type { StreamState } from "./native-to-pi/events.js";
/** Stable Native item identities shared by history replay and live notification projection. */
export function completedItemRevisions(items: unknown[]): Record<string, number> {
  const revisions: Record<string, number> = Object.create(null);
  for (const value of items) {
    if (!value || typeof value !== "object") continue;
    const item = value as Record<string, unknown>;
    if (typeof item.id !== "string") continue;
    if (!["completed", "failed", "interrupted", "lost"].includes(String(item.state))) continue;
    if (typeof item.revision !== "number" || !Number.isSafeInteger(item.revision)) continue;
    revisions[item.id] = Math.max(revisions[item.id] ?? 0, item.revision);
  }
  return revisions;
}

export function notificationItemIdentity(method: string, params: unknown): { id: string; revision?: number } | undefined {
  if (!method.startsWith("item/") || !params || typeof params !== "object") return undefined;
  const payload = params as Record<string, unknown>;
  const item = payload.item as Record<string, unknown> | undefined;
  const id = item?.id ?? payload.itemId ?? payload.item_id;
  if (typeof id !== "string" || !id) return undefined;
  const revision = item?.revision;
  return { id, ...(typeof revision === "number" ? { revision } : {}) };
}

/** Keep the active text/thinking item out of finalized history so it owns one streaming row. */
export function runningAssistantItems(items: unknown[]): Array<Record<string, unknown>> {
  return items.filter((value): value is Record<string, unknown> => {
    if (!value || typeof value !== "object") return false;
    const envelope = value as Record<string, unknown>;
    const item = envelope.item as Record<string, unknown> | undefined;
    return (envelope.state === "running" || envelope.state === "waiting") &&
      (item?.type === "assistantMessage" || item?.type === "reasoning");
  });
}

/** Fold page overlap by stable identity; use canonical sequence only when every item supplies it. */
export function normalizeHistoryItems(items: unknown[]): unknown[] {
  const byId = new Map<string, Record<string, unknown>>();
  const order: Array<string | unknown> = [];
  for (const value of items) {
    const item = value && typeof value === "object" ? value as Record<string, unknown> : undefined;
    if (!item || typeof item.id !== "string") { order.push(value); continue; }
    const previous = byId.get(item.id);
    if (!previous) order.push(item.id);
    if (!previous || Number(item.revision ?? 0) >= Number(previous.revision ?? 0)) byId.set(item.id, item);
  }
  const result = order.map(value => typeof value === "string" && byId.has(value) ? byId.get(value) : value);
  if (result.every(value => value && typeof value === "object" && Number.isSafeInteger((value as Record<string, unknown>).seq))) {
    result.sort((a, b) => Number((a as Record<string, unknown>).seq) - Number((b as Record<string, unknown>).seq));
  }
  return result;
}

/** Native deltas apply to the birth item (baseRevision=1), not to persisted deltas. */
export function hydrateRunningText(state: StreamState, running: Array<Record<string, unknown>>): Record<string, { kind: "text" | "thinking"; prefix: string }> {
  const prefixes: Record<string, { kind: "text" | "thinking"; prefix: string }> = Object.create(null);
  for (const envelope of running) {
    const id = String(envelope.id);
    const item = envelope.item as Record<string, unknown>;
    const kind = item.type === "reasoning" ? "thinking" : "text";
    const prefix = String(item.text ?? "");
    prefixes[id] = { kind, prefix };
    if (textHydrationState(state).baseKnown.has(id) || textHydrationState(state).completed.has(id)) continue;
    const accumulator = kind === "text" ? state.assistantTextByItem : state.reasoningTextByItem;
    accumulator.set(id, prefix + (accumulator.get(id) ?? ""));
    textHydrationState(state).baseKnown.add(id);
    if (kind === "text") state.openAssistantItem = id;
    else state.openReasoningItem = id;
    state.imAssistantStreamOpen = true;
  }
  return prefixes;
}
