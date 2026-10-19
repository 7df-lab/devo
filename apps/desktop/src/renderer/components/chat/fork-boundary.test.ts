import { describe, expect, test } from "bun:test"
import type { ChatTurn } from "../../atoms/derived/session-chat"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { forkBoundaryAfterTurnIndex } from "./fork-boundary"

function turn(turnId: string, created: number): ChatTurn {
	const info: NativeItemEnvelope = {
		id: `user-${turnId}`,
		sessionId: "s",
		turnId,
		seq: 1,
		revision: 1,
		createdAt: new Date(created).toISOString(),
		updatedAt: new Date(created).toISOString(),
		state: "completed",
		item: { type: "userMessage", content: [{ type: "text", text: "hello" }], entry: "turnStart" },
	}
	return {
		id: turnId,
		turnId,
		userMessage: { info },
		assistantMessages: [],
	}
}

describe("forkBoundaryAfterTurnIndex", () => {
	test("returns -1 when session is not a fork", () => {
		expect(forkBoundaryAfterTurnIndex([turn("t1", 100)], undefined, undefined, 500)).toBe(-1)
	})

	test("matches explicit fork turn id", () => {
		const turns = [turn("t1", 100), turn("t2", 200), turn("t3", 300)]
		expect(forkBoundaryAfterTurnIndex(turns, "parent", "t2", 500)).toBe(1)
	})

	test("uses fork creation time for tip forks", () => {
		const turns = [turn("t1", 100), turn("t2", 200), turn("t3", 600)]
		expect(forkBoundaryAfterTurnIndex(turns, "parent", undefined, 500)).toBe(1)
	})
})
