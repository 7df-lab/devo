/**
 * First-party Desktop transcript unit: Native `ItemEnvelope` wire shape.
 * Atoms and chat views store this directly — no OpenCode Message/Part dual.
 */
export type NativeItemEnvelope = {
	id: string
	sessionId: string
	turnId: string
	seq: number
	revision: number
	createdAt: string
	updatedAt: string
	state: string
	item: Record<string, unknown>
}

export function nativeItemType(envelope: NativeItemEnvelope): string {
	const item = envelope.item
	return typeof item?.type === "string" ? item.type : ""
}

export function isUserMessageItem(envelope: NativeItemEnvelope): boolean {
	return nativeItemType(envelope) === "userMessage"
}

export function compareNativeItems(left: NativeItemEnvelope, right: NativeItemEnvelope): number {
	const bySeq = (left.seq ?? 0) - (right.seq ?? 0)
	if (bySeq !== 0) return bySeq
	const byCreated = String(left.createdAt ?? "").localeCompare(String(right.createdAt ?? ""))
	if (byCreated !== 0) return byCreated
	return left.id.localeCompare(right.id)
}

export function sortedNativeItems(items: Iterable<NativeItemEnvelope>): NativeItemEnvelope[] {
	return [...items].sort(compareNativeItems)
}

/** Paginate history aligned to userMessage boundaries (same UX budget as former message limit). */
export function recentNativeItems(
	items: NativeItemEnvelope[],
	limit: number | undefined,
): NativeItemEnvelope[] {
	const sorted = sortedNativeItems(items)
	if (limit === undefined || sorted.length <= limit) return sorted
	let start = sorted.length - limit
	while (start > 0 && !isUserMessageItem(sorted[start])) {
		start -= 1
	}
	return sorted.slice(start)
}

export function envelopeFromWire(value: Record<string, unknown>): NativeItemEnvelope | null {
	const id = String(value.id ?? "")
	const sessionId = String(value.sessionId ?? "")
	const item = value.item
	if (!id || !sessionId || !item || typeof item !== "object") return null
	return {
		id,
		sessionId,
		turnId: String(value.turnId ?? ""),
		seq: Number(value.seq ?? 0),
		revision: Number(value.revision ?? 0),
		createdAt: String(value.createdAt ?? ""),
		updatedAt: String(value.updatedAt ?? value.createdAt ?? ""),
		state: String(value.state ?? "running"),
		item: item as Record<string, unknown>,
	}
}

/** Fold a newer envelope onto an existing cache entry by revision. */
export function mergeNativeEnvelope(
	existing: NativeItemEnvelope | undefined,
	incoming: NativeItemEnvelope,
): NativeItemEnvelope {
	if (!existing) return incoming
	if ((incoming.revision ?? 0) < (existing.revision ?? 0)) return existing
	return {
		...existing,
		...incoming,
		item: { ...existing.item, ...incoming.item },
	}
}

export function userMessageText(envelope: NativeItemEnvelope): string {
	const content = envelope.item.content
	if (!Array.isArray(content)) return ""
	return content
		.map((part) => (part && typeof part === "object" ? (part as Record<string, unknown>) : null))
		.filter((part): part is Record<string, unknown> => Boolean(part))
		.filter((part) => part.type === "text")
		.map((part) => String(part.text ?? ""))
		.join("\n")
}

export function assistantOrReasoningText(envelope: NativeItemEnvelope): string {
	return typeof envelope.item.text === "string" ? envelope.item.text : ""
}
