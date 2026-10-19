import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import {
	compareNativeItems,
	isUserMessageItem,
	sortedNativeItems,
} from "@devo-ai/sdk/v2/client"

const MAX_ITEMS_PER_SESSION = 200

export type SessionItem = NativeItemEnvelope

/** Ordered Native ItemEnvelope list for a session (source of truth for transcript). */
export const itemsFamily = atomFamily((_sessionId: string) => atom<SessionItem[]>([]))

export const setItemsAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			items: SessionItem[]
		},
	) => {
		const existing = get(itemsFamily(args.sessionId))
		if (!existing || existing.length === 0) {
			set(itemsFamily(args.sessionId), sortedNativeItems(args.items))
			return
		}
		const byId = new Map(args.items.map((item) => [item.id, item]))
		for (const item of existing) {
			byId.set(item.id, item)
		}
		set(itemsFamily(args.sessionId), sortedNativeItems(byId.values()))
	},
)

export const upsertItemAtom = atom(null, (get, set, item: SessionItem) => {
	const sessionId = item.sessionId
	let existing = get(itemsFamily(sessionId))

	// The canonical user item can arrive before queue/push returns. In that
	// order, the late optimistic bubble is already represented by its turn.
	if (
		isUserMessageItem(item) &&
		item.id.startsWith("optimistic-") &&
		item.turnId &&
		existing.some(
			(m) => isUserMessageItem(m) && !m.id.startsWith("optimistic-") && m.turnId === item.turnId,
		)
	) {
		return
	}

	if (isUserMessageItem(item) && !item.id.startsWith("optimistic-")) {
		const optimisticIndex = existing.findIndex(
			(m) =>
				m.id.startsWith("optimistic-") &&
				isUserMessageItem(m) &&
				(!m.turnId || m.turnId === item.turnId),
		)
		if (optimisticIndex !== -1) {
			existing = existing.filter((_, index) => index !== optimisticIndex)
		}
	}

	const index = existing.findIndex((m) => m.id === item.id)
	const updated =
		index >= 0
			? existing.map((m, i) => (i === index ? { ...m, ...item, item: { ...m.item, ...item.item } } : m))
			: [...existing, item]

	updated.sort(compareNativeItems)

	// Keep the live tail bounded until history has been expanded. Once a
	// paginated history fetch hydrates more than the live cap, do not discard
	// those requested rows each time a streaming update arrives.
	if (existing.length <= MAX_ITEMS_PER_SESSION) {
		while (updated.length > MAX_ITEMS_PER_SESSION) {
			updated.shift()
		}
	}

	set(itemsFamily(sessionId), updated)
})

export const removeItemAtom = atom(
	null,
	(get, set, args: { sessionId: string; itemId: string }) => {
		const existing = get(itemsFamily(args.sessionId))
		set(
			itemsFamily(args.sessionId),
			existing.filter((item) => item.id !== args.itemId),
		)
	},
)
