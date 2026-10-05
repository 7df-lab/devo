import { describe, expect, test } from "bun:test"
import { processEvent } from "./actions/event-processor"
import { sessionNativeFamily } from "./session-native"
import { sessionFamily, upsertSessionAtom } from "./sessions"
import { appStore } from "./store"
import { streamingVersionFamily } from "./streaming"

describe("Native session renderer state", () => {
	test("deduplicates replayed Native approvals by approval id", () => {
		const sessionID = "session-native-approval-dedup"
		appStore.set(upsertSessionAtom, {
			session: { id: sessionID, title: "Approval replay" },
			directory: "/repo",
		})
		const event = {
			type: "permission.asked",
			properties: {
				id: "approval-1",
				sessionId: sessionID,
				permission: "Run command",
			},
		} as const

		processEvent(event)
		processEvent(event)

		expect(appStore.get(sessionFamily(sessionID))?.permissions).toEqual([event.properties])
	})

	test("stores command, config, mode, usage, and occupancy updates from events", () => {
		const sessionID = "session-native-state"

		processEvent({
			type: "session.commands.updated",
			properties: {
				sessionId: sessionID,
				commands: [{ name: "compact", description: "Compact session" }],
			},
		})
		processEvent({
			type: "session.config.updated",
			properties: {
				sessionId: sessionID,
				configOptions: [{ id: "model", currentValue: "test-model" }],
			},
		})
		processEvent({
			type: "session.mode.updated",
			properties: {
				sessionId: sessionID,
				modeID: "plan",
			},
		})
		processEvent({
			type: "session.usage.updated",
			properties: {
				sessionId: sessionID,
				used: 42,
				size: 100,
				cost: { amount: 1, currency: "USD" },
			},
		})
		processEvent({
			type: "context.usage.updated",
			properties: {
				sessionId: sessionID,
				occupancy: {
					totalTokens: 48_000,
					contextWindowTokens: 190_000,
					categories: [
						{ id: "base", tokens: 8_000, shareBps: 1667 },
						{ id: "conversation", tokens: 40_000, shareBps: 8333 },
					],
				},
			},
		})

		expect(appStore.get(sessionNativeFamily(sessionID))).toEqual({
			commands: [{ name: "compact", description: "Compact session" }],
			configOptions: [{ id: "model", currentValue: "test-model" }],
			modeID: "plan",
			usage: {
				used: 48_000,
				size: 190_000,
				cost: { amount: 1, currency: "USD" },
			},
			occupancy: {
				totalTokens: 48_000,
				contextWindowTokens: 190_000,
				categories: [
					{ id: "base", tokens: 8_000, shareBps: 1667 },
					{ id: "conversation", tokens: 40_000, shareBps: 8333 },
				],
			},
		})
	})

	test("live turn usage advances the fill amount and syncs the window", () => {
		const sessionID = "session-native-usage-ahead"
		processEvent({
			type: "context.usage.updated",
			properties: {
				sessionId: sessionID,
				occupancy: {
					totalTokens: 16_700,
					contextWindowTokens: 190_000,
					categories: [
						{ id: "base", tokens: 10_000, shareBps: 5988 },
						{ id: "conversation", tokens: 6_700, shareBps: 4012 },
					],
				},
			},
		})
		processEvent({
			type: "session.usage.updated",
			properties: {
				sessionId: sessionID,
				used: 48_000,
				size: 250_000,
			},
		})

		const native = appStore.get(sessionNativeFamily(sessionID))
		expect(native.usage?.used).toBe(48_000)
		// Denominator follows the live effective window from the server.
		expect(native.usage?.size).toBe(250_000)
		expect(native.occupancy?.contextWindowTokens).toBe(250_000)
		expect(native.occupancy?.totalTokens).toBe(16_700)
	})

	test("applies occupancy window increases immediately", () => {
		const sessionID = "session-native-window-increase"
		processEvent({
			type: "context.usage.updated",
			properties: {
				sessionId: sessionID,
				occupancy: {
					totalTokens: 50_000,
					contextWindowTokens: 190_000,
					categories: [],
				},
			},
		})
		processEvent({
			type: "context.usage.updated",
			properties: {
				sessionId: sessionID,
				occupancy: {
					totalTokens: 52_000,
					contextWindowTokens: 1_000_000,
					categories: [],
				},
			},
		})

		const native = appStore.get(sessionNativeFamily(sessionID))
		expect(native.occupancy?.contextWindowTokens).toBe(1_000_000)
		expect(native.usage?.used).toBe(52_000)
		expect(native.usage?.size).toBe(1_000_000)
	})

	test("notifies session chat renders when Native items update", () => {
		const sessionID = "session-text-part-update"
		const initialVersion = appStore.get(streamingVersionFamily(sessionID))

		processEvent({
			type: "item.updated",
			properties: {
				info: {
					id: "message-text-part-update",
					sessionId: sessionID,
					turnId: "turn-1",
					seq: 1,
					revision: 1,
					createdAt: "2026-01-01T00:00:01.000Z",
					updatedAt: "2026-01-01T00:00:01.000Z",
					state: "running",
					item: { type: "assistantMessage", text: "streamed text" },
				},
			},
		})

		expect(appStore.get(streamingVersionFamily(sessionID))).toBe(initialVersion + 1)
	})

	test("bumps version on every Native item upsert", () => {
		const sessionID = "session-buffered-text-part"
		const versionBefore = appStore.get(streamingVersionFamily(sessionID))

		processEvent({
			type: "item.updated",
			properties: {
				info: {
					id: "buffered-text",
					sessionId: sessionID,
					turnId: "turn-1",
					seq: 1,
					revision: 1,
					createdAt: "2026-01-01T00:00:01.000Z",
					updatedAt: "2026-01-01T00:00:01.000Z",
					state: "running",
					item: { type: "assistantMessage", text: "hello world" },
				},
			},
		})

		expect(appStore.get(streamingVersionFamily(sessionID))).toBe(versionBefore + 1)
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
				sessionId: sessionID,
				turnId: "turn-1",
				attempt: 2,
				backoffMs: 1000,
				provider: "openai",
				model: "test-model",
				phase: "scheduled",
				message: "Internal server error",
			},
		})

		const scheduled = appStore.get(sessionFamily(sessionID))
		expect(scheduled?.retryStatus).toEqual({
			turnId: "turn-1",
			attempt: 2,
			backoffMs: 1000,
			provider: "openai",
			model: "test-model",
			phase: "scheduled",
			message: "Internal server error",
		})
		expect(scheduled?.providerErrors).toEqual([
			{
				id: "retry-turn-1-2",
				turnId: "turn-1",
				message: "Internal server error",
				phase: "scheduled",
				attempt: 2,
				backoffMs: 1000,
				scheduledAtMs: scheduled?.providerErrors?.[0]?.scheduledAtMs,
			},
		])
		expect(typeof scheduled?.providerErrors?.[0]?.scheduledAtMs).toBe("number")

		processEvent({
			type: "turn.provider_retry_status",
			properties: {
				sessionId: sessionID,
				turnId: "turn-1",
				attempt: 2,
				backoffMs: 0,
				provider: "openai",
				model: "test-model",
				phase: "resumed",
				message: "Internal server error",
			},
		})
		processEvent({
			type: "session.error",
			properties: {
				sessionId: sessionID,
				error: {
					name: "PROVIDER_SERVER_ERROR",
					data: { message: "Internal server error" },
				},
			},
		})

		const failed = appStore.get(sessionFamily(sessionID))
		expect(failed).toEqual({
			session: { id: sessionID, title: "Retry test" },
			directory: "/repo",
			status: { type: "idle" },
			permissions: [],
			questions: [],
			retryStatus: undefined,
			providerErrors: [
				{
					id: "retry-turn-1-2",
					turnId: "turn-1",
					message: "Internal server error",
					phase: "scheduled",
					attempt: 2,
					backoffMs: 1000,
					scheduledAtMs: failed?.providerErrors?.[0]?.scheduledAtMs,
				},
				{
					id: "failed-turn-1-PROVIDER_SERVER_ERROR-Internal server error",
					turnId: "turn-1",
					message: "Internal server error",
					phase: "failed",
					code: "PROVIDER_SERVER_ERROR",
				},
			],
			error: {
				name: "PROVIDER_SERVER_ERROR",
				data: { message: "Internal server error" },
			},
		})
	})
})
