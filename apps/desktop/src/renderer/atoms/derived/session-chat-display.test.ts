import { describe, expect, test } from "bun:test"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { itemDisplayText } from "./session-chat"

const warning: NativeItemEnvelope = {
	id: "warning-1",
	sessionId: "session-1",
	turnId: "turn-1",
	seq: 1,
	revision: 1,
	createdAt: "2026-10-09T00:00:00Z",
	updatedAt: "2026-10-09T00:00:00Z",
	state: "completed",
	item: {
		type: "warning",
		code: "rlmKernelUnfenced",
		message: "The sandbox is unavailable.",
		retryable: false,
	},
}

describe("Native warning text", () => {
	test("shows the actual runtime message instead of the item discriminator", () => {
		expect(itemDisplayText(warning)).toBe("The sandbox is unavailable.")
	})
	test("uses readable fallback text for an empty message", () => {
		expect(itemDisplayText({ ...warning, item: { ...warning.item, message: "  " } })).toBe(
			"Runtime warning.",
		)
	})
})
