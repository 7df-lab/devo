import { afterEach, describe, expect, test } from "bun:test"
import type { ChatTurn } from "../atoms/derived/session-chat"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import {
	computeLatestTurnTimerSplit,
	computeThoughtWorkTime,
	computeTurnWorkTime,
	formatWorkDuration,
} from "./session-metrics"

const originalNow = Date.now

afterEach(() => {
	Date.now = originalNow
})

function iso(ms: number): string {
	return new Date(ms).toISOString()
}

function userEntry(id: string, createdMs: number, turnId = "t1"): ChatTurn["userMessage"] {
	const info: NativeItemEnvelope = {
		id,
		sessionId: "s1",
		turnId,
		seq: 1,
		revision: 1,
		createdAt: iso(createdMs),
		updatedAt: iso(createdMs),
		state: "completed",
		item: { type: "userMessage", content: [{ type: "text", text: "hi" }], entry: "turnStart" },
	}
	return { info }
}

function assistantEntry(
	id: string,
	createdMs: number,
	updatedMs: number,
	state: string = "completed",
	turnId = "t1",
): ChatTurn["assistantMessages"][number] {
	const info: NativeItemEnvelope = {
		id,
		sessionId: "s1",
		turnId,
		seq: 2,
		revision: 1,
		createdAt: iso(createdMs),
		updatedAt: iso(updatedMs),
		state,
		item: { type: "assistantMessage", text: "ok" },
	}
	return { info }
}

function turnWith(
	assistantMessages: ChatTurn["assistantMessages"],
	userCreated = 1_000,
	id = "u1",
): ChatTurn {
	return {
		id,
		turnId: "t1",
		userMessage: userEntry(id, userCreated),
		assistantMessages,
	}
}

function formatTimerSplit(
	split: { completedMs: number; activeStartMs: number | null },
	now: number,
): string {
	return formatWorkDuration(split.completedMs + (split.activeStartMs != null ? now - split.activeStartMs : 0))
}

describe("turn duration metrics", () => {
	test("computes completed turn duration from user message to assistant completion", () => {
		const turn = turnWith([assistantEntry("a1", 2_000, 61_000)])
		expect(computeTurnWorkTime(turn)).toBe(60_000)
	})

	test("falls back to latest envelope updatedAt when needed", () => {
		const turn = turnWith([assistantEntry("a1", 2_000, 9_000)])
		expect(computeTurnWorkTime(turn, { now: () => 99_000 })).toBe(8_000)
	})

	test("uses active now for in-progress turns", () => {
		const turn = turnWith([assistantEntry("a1", 2_000, 2_000, "running")])
		expect(computeTurnWorkTime(turn, { active: true, now: () => 11_000 })).toBe(10_000)
	})
})

describe("thought duration metrics", () => {
	test("computes completed thought duration from start/end", () => {
		expect(computeThoughtWorkTime({ time: { start: 1_000, end: 4_000 } })).toBe(3_000)
	})

	test("uses now for active thoughts", () => {
		expect(computeThoughtWorkTime({ time: { start: 1_000 } }, { active: true, now: () => 2_500 })).toBe(
			1_500,
		)
	})
})

describe("top bar turn timer metrics", () => {
	test("runs from the latest user message createdAt", () => {
		const start = 1_700_000_000_000
		const turn = turnWith([], start, "optimistic-1")
		const split = computeLatestTurnTimerSplit([turn], { mode: "running", now: () => start + 5_000 })
		expect({ split, label: formatTimerSplit(split, start + 5_000) }).toEqual({
			split: { completedMs: 0, activeStartMs: start },
			label: "5s",
		})
	})

	test("uses only the latest user turn so the timer resets on the next message", () => {
		const start = 1_782_388_920_000
		const nextStart = start + 60_000
		const prior = turnWith([assistantEntry("a1", start + 1_000, start + 30_000)], start, "optimistic-1")
		const nextTurn = turnWith([], nextStart, "optimistic-2")
		const split = computeLatestTurnTimerSplit([prior, nextTurn], {
			mode: "running",
			now: () => nextStart + 3_000,
		})
		expect({ split, label: formatTimerSplit(split, nextStart + 3_000) }).toEqual({
			split: { completedMs: 0, activeStartMs: nextStart },
			label: "3s",
		})
	})

	test("stops on the completed turn timestamp when the session becomes idle", () => {
		const start = 1_700_000_000_000
		const turn = turnWith([assistantEntry("a1", start + 1_000, start + 42_000)], start)
		const split = computeLatestTurnTimerSplit([turn], { mode: "stopped" })
		expect({ split, label: formatTimerSplit(split, start + 99_000) }).toEqual({
			split: { completedMs: 42_000, activeStartMs: null },
			label: "42s",
		})
	})

	test("prefers the completed turn timestamp over a stale live fallback", () => {
		const start = 1_700_000_000_000
		const turn = turnWith([assistantEntry("a1", start + 1_000, start + 42_000)], start)
		const split = computeLatestTurnTimerSplit([turn], {
			mode: "stopped",
			fallbackCompletedMs: 99_000,
		})
		expect({ split, label: formatTimerSplit(split, start + 120_000) }).toEqual({
			split: { completedMs: 42_000, activeStartMs: null },
			label: "42s",
		})
	})

	test("keeps the last live elapsed value when an interrupted turn has no completion timestamp", () => {
		const start = 1_700_000_000_000
		const turn = turnWith([assistantEntry("a1", start + 1_000, start + 1_000, "running")], start)
		const split = computeLatestTurnTimerSplit([turn], {
			mode: "stopped",
			fallbackCompletedMs: 12_000,
		})
		expect(split).toEqual({ completedMs: 12_000, activeStartMs: null })
	})

	test("does not start a huge live timer from historical ordering timestamps", () => {
		const start = 1
		const turn = turnWith([], start)
		const split = computeLatestTurnTimerSplit([turn], {
			mode: "running",
			now: () => 1_700_000_000_000,
		})
		expect(split.activeStartMs).toBeNull()
	})
})
