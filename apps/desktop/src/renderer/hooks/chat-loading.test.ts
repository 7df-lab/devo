import { describe, expect, test } from "bun:test"
import { shouldShowChatLoadingSkeleton } from "./chat-loading"

describe("chat loading skeleton", () => {
	test("shows only during an empty initial load", () => {
		expect([
			shouldShowChatLoadingSkeleton(true, 0),
			shouldShowChatLoadingSkeleton(true, 1),
			shouldShowChatLoadingSkeleton(false, 0),
		]).toEqual([true, false, false])
	})
})
