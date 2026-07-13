import { describe, expect, test } from "bun:test"
import { processEvent } from "./actions/event-processor"
import { partsFamily, partStorageKey } from "./parts"
import { sessionAcpFamily } from "./session-acp"
import { sessionFamily, upsertSessionAtom } from "./sessions"
import { appStore } from "./store"
import { streamingVersionFamily } from "./streaming"

describe("ACP session renderer state", () => {
	test("stores command, config, mode, and usage updates from events", () => {
		const sessionID = "session-acp-state"

		processEvent({
			type: "session.commands.updated",
			properties: {
				sessionID,
				commands: [{ name: "compact", description: "Compact session" }],
			},
		})
		processEvent({
			type: "session.config.updated",
			properties: {
				sessionID,
				configOptions: [{ id: "model", currentValue: "test-model" }],
			},
		})
		processEvent({
			type: "session.mode.updated",
			properties: {
				sessionID,
				modeID: "plan",
			},
		})
		processEvent({
			type: "session.usage.updated",
			properties: {
				sessionID,
				used: 42,
				size: 100,
				cost: { amount: 1, currency: "USD" },
			},
		})

		expect(appStore.get(sessionAcpFamily(sessionID))).toEqual({
			commands: [{ name: "compact", description: "Compact session" }],
			configOptions: [{ id: "model", currentValue: "test-model" }],
			modeID: "plan",
			usage: {
				used: 42,
				size: 100,
				cost: { amount: 1, currency: "USD" },
			},
		})
	})

	test("notifies session chat renders when text parts update", () => {
		const sessionID = "session-text-part-update"
		const messageID = "message-text-part-update"
		const initialVersion = appStore.get(streamingVersionFamily(sessionID))

		processEvent({
			type: "message.part.updated",
			properties: {
				part: {
					id: "message-text-part-update-text",
					sessionID,
					messageID,
					type: "text",
					text: "streamed text",
					time: { start: 1, end: 1 },
				},
			},
		})

		expect(appStore.get(partsFamily(partStorageKey(sessionID, messageID)))).toEqual([
			{
				id: "message-text-part-update-text",
				sessionID,
				messageID,
				type: "text",
				text: "streamed text",
				time: { start: 1, end: 1 },
			},
		])
		expect(appStore.get(streamingVersionFamily(sessionID))).toBe(initialVersion + 1)
	})

	test("stores scheduled retries, clears resumed retries, and reports transient failures", () => {
		const sessionID = "session-provider-retry"
		appStore.set(upsertSessionAtom, {
			session: { id: sessionID, title: "Retry test" },
			directory: "/repo",
		})

		processEvent({
			type: "turn.provider_retry_status",
			properties: {
				sessionID,
				turnID: "turn-1",
				attempt: 2,
				backoffMs: 1000,
				provider: "openai",
				model: "test-model",
				phase: "scheduled",
				message: "Retrying provider request in 1.0s",
			},
		})

		expect(appStore.get(sessionFamily(sessionID))?.retryStatus).toEqual({
			turnId: "turn-1",
			attempt: 2,
			backoffMs: 1000,
			provider: "openai",
			model: "test-model",
			phase: "scheduled",
			message: "Retrying provider request in 1.0s",
		})

		processEvent({
			type: "turn.provider_retry_status",
			properties: {
				sessionID,
				turnID: "turn-1",
				attempt: 2,
				backoffMs: 0,
				provider: "openai",
				model: "test-model",
				phase: "resumed",
				message: "Retrying provider request now",
			},
		})
		processEvent({
			type: "session.error",
			properties: {
				sessionID,
				error: {
					name: "PROVIDER_SERVER_ERROR",
					data: { message: "Internal server error" },
				},
			},
		})

		expect(appStore.get(sessionFamily(sessionID))).toEqual({
			session: { id: sessionID, title: "Retry test" },
			directory: "/repo",
			status: { type: "idle" },
			permissions: [],
			questions: [],
			retryStatus: undefined,
			error: {
				name: "PROVIDER_SERVER_ERROR",
				data: { message: "Internal server error" },
			},
		})
	})
})
