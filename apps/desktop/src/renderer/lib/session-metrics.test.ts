import { afterEach, describe, expect, test } from "bun:test"
import type { ChatTurn } from "../atoms/derived/session-chat"
import type { Message } from "./types"
import { computeSessionMetrics, computeTurnWorkTime, computeTurnWorkTimeSplit } from "./session-metrics"

const originalNow = Date.now

afterEach(() => {
	Date.now = originalNow
})

function turnWith(assistantMessages: ChatTurn["assistantMessages"]): ChatTurn {
	return {
		id: "u1",
		userMessage: {
			info: { id: "u1", role: "user", time: { created: 1_000 } },
			parts: [],
		},
		assistantMessages,
	} as ChatTurn
}

describe("turn duration metrics", () => {
	test("computes completed turn duration from user message to assistant completion", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000, completed: 61_000 } },
				parts: [],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn)).toBe(60_000)
	})

	test("falls back to latest part timestamp when assistant completion is missing", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [
					{
						id: "tool-1",
						type: "tool",
						state: { status: "completed", time: { start: 3_000, end: 9_000 } },
					},
				],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn, { now: () => 99_000 })).toBe(8_000)
	})

	test("uses Date.now only for active turns", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [],
			},
		] as ChatTurn["assistantMessages"])

		expect({
			completed: computeTurnWorkTime(turn, { now: () => 99_000 }),
			active: computeTurnWorkTime(turn, { active: true, now: () => 11_000 }),
			split: computeTurnWorkTimeSplit(turn),
		}).toEqual({
			completed: 1_000,
			active: 10_000,
			split: { completedMs: 0, activeStartMs: 1_000 },
		})
	})

	test("drops implausible completed duration from incompatible historical timestamps", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [
					{
						id: "tool-1",
						type: "tool",
						state: { status: "completed", time: { start: 3_000, end: 200_000_000 } },
					},
				],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn)).toBe(0)
	})

	test("does not treat historical ordering timestamps as active session timers", () => {
		Date.now = () => Date.parse("2026-06-25T12:00:00.000Z")
		const messages = [
			{ id: "history-0", role: "user", time: { created: 1 } },
			{
				id: "history-1",
				role: "assistant",
				parentID: "history-0",
				time: { created: 2 },
			},
		] as Message[]

		expect(computeSessionMetrics(messages)).toEqual({
			workTimeMs: 0,
			completedWorkTimeMs: 0,
			activeStartMs: null,
			cost: 0,
			tokens: {
				input: 0,
				output: 0,
				reasoning: 0,
				cacheRead: 0,
				cacheWrite: 0,
				total: 0,
			},
			exchangeCount: 1,
			userMessageCount: 1,
			assistantMessageCount: 1,
			modelDistribution: {},
			cacheEfficiency: 0,
			errorCount: 0,
			avgExchangeCost: 0,
			avgExchangeTimeMs: 0,
		})
	})
})
