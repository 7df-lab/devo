import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { Part } from "../lib/types"

const PART_STORAGE_SEPARATOR = "\u001f"

export function partStorageKey(sessionId: string, messageId: string): string {
	return `${sessionId}${PART_STORAGE_SEPARATOR}${messageId}`
}

function binarySearch<T>(
	arr: T[],
	target: string,
	key: (item: T) => string,
): { found: boolean; index: number } {
	let lo = 0
	let hi = arr.length
	while (lo < hi) {
		const mid = (lo + hi) >>> 1
		const cmp = key(arr[mid]).localeCompare(target)
		if (cmp < 0) lo = mid + 1
		else if (cmp > 0) hi = mid
		else return { found: true, index: mid }
	}
	return { found: false, index: lo }
}

// ============================================================
// Per-message part list (sorted by id)
// ============================================================

export const partsFamily = atomFamily((_messageId: string) => atom<Part[]>([]))

function upsertPart(existing: Part[] | undefined, part: Part): Part[] {
	if (!existing || existing.length === 0) {
		return [part]
	}

	const result = binarySearch(existing, part.id, (p) => p.id)
	if (result.found) {
		if (existing[result.index] === part) return existing
		const updated = existing.slice()
		updated[result.index] = part
		return updated
	}

	const updated = existing.slice()
	updated.splice(result.index, 0, part)
	return updated
}

function applyPartDelta(
	existing: Part[] | undefined,
	partId: string,
	field: string,
	delta: string,
): Part[] | undefined {
	if (!existing || existing.length === 0) return undefined
	const result = binarySearch(existing, partId, (p) => p.id)
	if (!result.found) return undefined
	const part = existing[result.index]
	const record = part as Record<string, unknown>
	const current = record[field]
	const updated = existing.slice()
	updated[result.index] = {
		...part,
		[field]: (typeof current === "string" ? current : "") + delta,
	}
	return updated
}

function removePart(existing: Part[] | undefined, partId: string): Part[] | undefined {
	if (!existing) return undefined
	const result = binarySearch(existing, partId, (p) => p.id)
	if (!result.found) return undefined
	const updated = existing.slice()
	updated.splice(result.index, 1)
	return updated
}

// ============================================================
// Action atoms
// ============================================================

/** Upsert a single part */
export const upsertPartAtom = atom(null, (get, set, part: Part) => {
	const scopedKey = partStorageKey(part.sessionID, part.messageID)
	set(partsFamily(scopedKey), upsertPart(get(partsFamily(scopedKey)), part))
	set(partsFamily(part.messageID), upsertPart(get(partsFamily(part.messageID)), part))
})

/** Batch upsert multiple parts (used when flushing streaming buffer) */
export const batchUpsertPartsAtom = atom(null, (get, set, parts: Part[]) => {
	if (parts.length === 0) return

	// Group by messageId to minimize atom writes
	const byMessage = new Map<string, Part[]>()
	for (const part of parts) {
		const group = byMessage.get(part.messageID) ?? []
		group.push(part)
		byMessage.set(part.messageID, group)
	}

	for (const [messageId, messageParts] of byMessage) {
		let legacy = get(partsFamily(messageId))
		for (const part of messageParts) {
			const scopedKey = partStorageKey(part.sessionID, part.messageID)
			set(partsFamily(scopedKey), upsertPart(get(partsFamily(scopedKey)), part))
			legacy = upsertPart(legacy, part)
		}
		set(partsFamily(messageId), legacy)
	}
})

/** Apply a string delta to a specific field of an existing part */
export const applyPartDeltaAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			messageId: string
			partId: string
			field: string
			delta: string
		},
	) => {
		const scopedKey = partStorageKey(args.sessionId, args.messageId)
		const scoped = applyPartDelta(
			get(partsFamily(scopedKey)),
			args.partId,
			args.field,
			args.delta,
		)
		if (scoped) set(partsFamily(scopedKey), scoped)

		const legacy = applyPartDelta(
			get(partsFamily(args.messageId)),
			args.partId,
			args.field,
			args.delta,
		)
		if (legacy) set(partsFamily(args.messageId), legacy)
	},
)

/** Remove a part from a message */
export const removePartAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			messageId: string
			partId: string
		},
	) => {
		const scopedKey = partStorageKey(args.sessionId, args.messageId)
		const scoped = removePart(get(partsFamily(scopedKey)), args.partId)
		if (scoped) set(partsFamily(scopedKey), scoped)

		const legacy = removePart(get(partsFamily(args.messageId)), args.partId)
		if (legacy) set(partsFamily(args.messageId), legacy)
	},
)
