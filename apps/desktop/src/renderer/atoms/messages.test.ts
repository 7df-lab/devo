import { describe, expect, test } from "bun:test"
import { createStore } from "jotai"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { groupIntoTurns, mergeSessionItems } from "./derived/session-chat"
import { itemsFamily, setItemsAtom, upsertItemAtom } from "./messages"

function userItem(id: string, turnId: string, seq: number, createdAt: string): NativeItemEnvelope {
	return {
		id,
		sessionId: "s1",
		turnId,
		seq,
		revision: 1,
		createdAt,
		updatedAt: createdAt,
		state: "completed",
		item: { type: "userMessage", content: [{ type: "text", text: "hi" }], entry: "turnStart" },
	}
}

function assistantItem(id: string, turnId: string, seq: number, createdAt: string): NativeItemEnvelope {
	return {
		id,
		sessionId: "s1",
		turnId,
		seq,
		revision: 1,
		createdAt,
		updatedAt: createdAt,
		state: "completed",
		item: { type: "assistantMessage", text: "hello" },
	}
}

describe("Native item ordering", () => {
	for (const canonicalFirst of [true, false]) {
		test(`reconciles user messages when canonical arrives ${canonicalFirst ? "first" : "last"}`, () => {
			const store = createStore()
			const canonical = userItem("u1", "t1", 1, "2026-01-01T00:00:01.000Z")
			const optimistic = userItem("optimistic-1", "t1", Number.MAX_SAFE_INTEGER, "2026-01-01T00:00:02.000Z")
			for (const item of canonicalFirst ? [canonical, optimistic] : [optimistic, canonical]) {
				store.set(upsertItemAtom, item)
			}
			expect(store.get(itemsFamily("s1"))).toEqual([canonical])
		})
	}

	test("keeps identical prompts in distinct turns", () => {
		const store = createStore()
		const canonical = userItem("u1", "t1", 1, "2026-01-01T00:00:01.000Z")
		const optimistic = userItem("optimistic-2", "t2", Number.MAX_SAFE_INTEGER, "2026-01-01T00:00:02.000Z")
		store.set(upsertItemAtom, canonical)
		store.set(upsertItemAtom, optimistic)
		expect(store.get(itemsFamily("s1"))).toEqual([canonical, optimistic])
	})

	test("keeps assistant replies after optimistic user messages by seq", () => {
		const store = createStore()
		const user = userItem("optimistic-2000", "", 1, "2026-01-01T00:00:02.000Z")
		const assistant = assistantItem("a1", "turn-1", 2, "2026-01-01T00:00:02.100Z")

		store.set(upsertItemAtom, user)
		store.set(upsertItemAtom, assistant)

		const items = store.get(itemsFamily("s1"))
		const entries = mergeSessionItems(items)

		expect(items.map((item) => item.id)).toEqual([user.id, assistant.id])
		expect(groupIntoTurns(entries, [])).toEqual([
			{
				id: user.id,
				turnId: undefined,
				userMessage: { info: user },
				assistantMessages: [{ info: assistant }],
			},
		])
	})

	test("propagates protocol turn ids onto chat turns", () => {
		const user = userItem("u1", "protocol-turn-1", 1, "2026-01-01T00:00:01.000Z")
		const assistant = assistantItem("a1", "protocol-turn-1", 2, "2026-01-01T00:00:02.000Z")
		const turns = groupIntoTurns([{ info: user }, { info: assistant }], [])

		expect(turns).toEqual([
			{
				id: "u1",
				turnId: "protocol-turn-1",
				userMessage: { info: user },
				assistantMessages: [{ info: assistant }],
			},
		])
	})

	test("hydrates session items without a Part dual", () => {
		const store = createStore()
		const first = userItem("m1", "t1", 1, "2026-01-01T00:00:01.000Z")
		store.set(setItemsAtom, { sessionId: "s1", items: [first] })
		expect(store.get(itemsFamily("s1"))).toEqual([first])
	})

	for (const coldReplay of [false, true]) {
		test(`keeps steering messages after completion (${coldReplay ? "cold replay" : "live events"})`, () => {
			const store = createStore()
			const user = userItem("u1", "t1", 1, "2026-01-01T00:00:01.000Z")
			const before = assistantItem("a1", "t1", 2, "2026-01-01T00:00:02.000Z")
			const steer = {
				...userItem("steer-1", "t1", 3, "2026-01-01T00:00:03.000Z"),
				item: {
					type: "userMessage",
					entry: "steer",
					content: [{ type: "text", text: "Keep this steering message visible." }],
				},
			}
			const after = assistantItem("a2", "t1", 4, "2026-01-01T00:00:04.000Z")
			const items = [user, before, steer, after]
			if (coldReplay) {
				store.set(setItemsAtom, { sessionId: "s1", items: [...items].reverse() })
			} else {
				for (const item of items) store.set(upsertItemAtom, item)
			}
			const turns = groupIntoTurns(mergeSessionItems(store.get(itemsFamily("s1"))), [])
			expect(turns).toEqual([
				{ id: user.id, turnId: "t1", userMessage: { info: user }, assistantMessages: [{ info: before }] },
				{ id: steer.id, turnId: "t1", userMessage: { info: steer }, assistantMessages: [{ info: after }] },
			])
			expect(groupIntoTurns(mergeSessionItems(store.get(itemsFamily("s1"))), turns)).toEqual(turns)
		})
	}

	test("keeps explicitly loaded earlier history during live updates", () => {
		const store = createStore()
		const history = Array.from({ length: 240 }, (_, index) =>
			assistantItem(`item-${index}`, `turn-${index}`, index + 1, "2026-01-01T00:00:00.000Z"),
		)
		store.set(setItemsAtom, { sessionId: "s1", items: history })
		store.set(upsertItemAtom, { ...history[239], revision: 2 })
		store.set(upsertItemAtom, assistantItem("new-item", "new-turn", 241, "2026-01-01T00:01:00.000Z"))

		const items = store.get(itemsFamily("s1"))
		expect(items).toHaveLength(241)
		expect(items[0]?.id).toBe("item-0")
		expect(items.at(-1)?.id).toBe("new-item")
	})

	test("groups turns once and skips orphan assistant items before a user message", () => {
		const orphan = assistantItem("orphan", "t0", 1, "2026-01-01T00:00:01.000Z")
		const firstUser = userItem("u1", "t1", 2, "2026-01-01T00:00:02.000Z")
		const firstAssistant = assistantItem("a1", "t1", 3, "2026-01-01T00:00:03.000Z")
		const turns = groupIntoTurns(
			[{ info: orphan }, { info: firstUser }, { info: firstAssistant }],
			[],
		)
		expect(turns).toEqual([
			{
				id: "u1",
				turnId: "t1",
				userMessage: { info: firstUser },
				assistantMessages: [{ info: firstAssistant }],
			},
		])
	})
})
