import { describe, expect, test } from "bun:test"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { isAssistantItemError } from "./assistant-item-error"

function envelope(
	type: string,
	state: string,
	extra: Record<string, unknown> = {},
): NativeItemEnvelope {
	return {
		id: "item-1",
		sessionId: "session-1",
		turnId: "turn-1",
		seq: 1,
		revision: 1,
		createdAt: "2026-01-01T00:00:00.000Z",
		updatedAt: "2026-01-01T00:00:00.000Z",
		state,
		item: { type, ...extra },
	}
}

describe("Native assistant error detection", () => {
	test("detects failed state and explicit assistant error metadata", () => {
		expect(isAssistantItemError(envelope("assistantMessage", "failed"))).toBe(true)
		expect(isAssistantItemError(envelope("assistantMessage", "completed", { error: "provider failed" }))).toBe(true)
	})

	test("does not treat successful assistants or failed tool rows as assistant errors", () => {
		expect(isAssistantItemError(envelope("assistantMessage", "completed"))).toBe(false)
		expect(isAssistantItemError(envelope("toolResult", "failed", { error: "tool failed" }))).toBe(false)
	})
})
