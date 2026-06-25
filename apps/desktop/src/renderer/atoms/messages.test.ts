import { describe, expect, test } from "bun:test"
import { createStore } from "jotai"
import { groupIntoTurns, mergeSessionParts } from "./derived/session-chat"
import { messagesFamily, setMessagesAtom, upsertMessageAtom } from "./messages"
import { partsFamily, partStorageKey } from "./parts"

describe("message ordering", () => {
	test("keeps assistant replies after optimistic user messages by creation time", () => {
		const store = createStore()
		const user = {
			id: "optimistic-2000",
			sessionID: "s1",
			role: "user",
			time: { created: 2_000 },
		}
		const assistant = {
			id: "019ef97e-0b70-7b20-88ee-d124de3aacde",
			sessionID: "s1",
			role: "assistant",
			parentID: user.id,
			time: { created: 2_100 },
		}

		store.set(upsertMessageAtom, user)
		store.set(upsertMessageAtom, assistant)

		const messages = store.get(messagesFamily("s1"))
		const entries = mergeSessionParts("s1", messages, () => [], 0)

		expect(messages.map((message) => message.id)).toEqual([user.id, assistant.id])
		expect(groupIntoTurns(entries, [])).toEqual([
			{
				id: user.id,
				userMessage: { info: user, parts: [] },
				assistantMessages: [{ info: assistant, parts: [] }],
			},
		])
	})

	test("keeps parts scoped when message IDs repeat across sessions", () => {
		const store = createStore()
		const messageId = "repeat-message"
		const firstMessage = {
			id: messageId,
			sessionID: "s1",
			role: "user",
			time: { created: 1 },
		}
		const secondMessage = {
			id: messageId,
			sessionID: "s2",
			role: "user",
			time: { created: 1 },
		}
		const firstPart = {
			id: "text",
			sessionID: "s1",
			messageID: messageId,
			type: "text",
			text: "first session",
			time: { start: 1 },
		}
		const secondPart = {
			id: "text",
			sessionID: "s2",
			messageID: messageId,
			type: "text",
			text: "second session",
			time: { start: 1 },
		}

		store.set(setMessagesAtom, {
			sessionId: "s1",
			messages: [firstMessage],
			parts: { [messageId]: [firstPart] },
		})
		store.set(setMessagesAtom, {
			sessionId: "s2",
			messages: [secondMessage],
			parts: { [messageId]: [secondPart] },
		})

		const firstEntries = mergeSessionParts(
			"s1",
			store.get(messagesFamily("s1")),
			(id) => store.get(partsFamily(partStorageKey("s1", id))),
			0,
		)
		const secondEntries = mergeSessionParts(
			"s2",
			store.get(messagesFamily("s2")),
			(id) => store.get(partsFamily(partStorageKey("s2", id))),
			0,
		)

		expect(firstEntries).toEqual([{ info: firstMessage, parts: [firstPart] }])
		expect(secondEntries).toEqual([{ info: secondMessage, parts: [secondPart] }])
	})

	test("groups turns once and skips orphan or mismatched assistant messages", () => {
		const orphanAssistant = {
			id: "orphan",
			sessionID: "s1",
			role: "assistant",
			time: { created: 1 },
		}
		const firstUser = {
			id: "u1",
			sessionID: "s1",
			role: "user",
			time: { created: 2 },
		}
		const firstAssistant = {
			id: "a1",
			sessionID: "s1",
			role: "assistant",
			parentID: "u1",
			time: { created: 3 },
		}
		const mismatchedAssistant = {
			id: "a-mismatch",
			sessionID: "s1",
			role: "assistant",
			parentID: "u2",
			time: { created: 4 },
		}
		const secondUser = {
			id: "u2",
			sessionID: "s1",
			role: "user",
			time: { created: 5 },
		}
		const secondAssistant = {
			id: "a2",
			sessionID: "s1",
			role: "assistant",
			time: { created: 6 },
		}
		const entries = [
			orphanAssistant,
			firstUser,
			firstAssistant,
			mismatchedAssistant,
			secondUser,
			secondAssistant,
		].map((info) => ({ info, parts: [] }))

		expect(groupIntoTurns(entries, [])).toEqual([
			{
				id: "u1",
				userMessage: { info: firstUser, parts: [] },
				assistantMessages: [{ info: firstAssistant, parts: [] }],
			},
			{
				id: "u2",
				userMessage: { info: secondUser, parts: [] },
				assistantMessages: [{ info: secondAssistant, parts: [] }],
			},
		])
	})

	test("keeps tool messages in their parent turn", () => {
		const firstUser = {
			id: "u1",
			sessionID: "s1",
			role: "user",
			time: { created: 1 },
		}
		const firstTool = {
			id: "tool-a",
			sessionID: "s1",
			role: "assistant",
			parentID: "u1",
			time: { created: 2 },
		}
		const firstAssistant = {
			id: "a1",
			sessionID: "s1",
			role: "assistant",
			parentID: "u1",
			time: { created: 3 },
		}
		const secondUser = {
			id: "u2",
			sessionID: "s1",
			role: "user",
			time: { created: 4 },
		}
		const secondTool = {
			id: "tool-b",
			sessionID: "s1",
			role: "assistant",
			parentID: "u2",
			time: { created: 5 },
		}
		const secondAssistant = {
			id: "a2",
			sessionID: "s1",
			role: "assistant",
			parentID: "u2",
			time: { created: 6 },
		}
		const firstToolPart = {
			id: "tool-a-part",
			sessionID: "s1",
			messageID: "tool-a",
			type: "tool",
			callID: "call-a",
			tool: "read",
			state: { status: "completed", input: {}, output: "", title: "Read A", metadata: {} },
		}
		const secondToolPart = {
			id: "tool-b-part",
			sessionID: "s1",
			messageID: "tool-b",
			type: "tool",
			callID: "call-b",
			tool: "read",
			state: { status: "completed", input: {}, output: "", title: "Read B", metadata: {} },
		}
		const entries = [
			{ info: firstUser, parts: [] },
			{ info: firstTool, parts: [firstToolPart] },
			{ info: firstAssistant, parts: [] },
			{ info: secondUser, parts: [] },
			{ info: secondTool, parts: [secondToolPart] },
			{ info: secondAssistant, parts: [] },
		]

		expect(groupIntoTurns(entries, [])).toEqual([
			{
				id: "u1",
				userMessage: { info: firstUser, parts: [] },
				assistantMessages: [
					{ info: firstTool, parts: [firstToolPart] },
					{ info: firstAssistant, parts: [] },
				],
			},
			{
				id: "u2",
				userMessage: { info: secondUser, parts: [] },
				assistantMessages: [
					{ info: secondTool, parts: [secondToolPart] },
					{ info: secondAssistant, parts: [] },
				],
			},
		])
	})
})
