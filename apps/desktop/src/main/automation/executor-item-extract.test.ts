import { describe, expect, test } from "bun:test"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { extractAutomationItem } from "./item-extract"

function envelope(
	overrides: Partial<NativeItemEnvelope> & { item: Record<string, unknown> },
): NativeItemEnvelope {
	return {
		id: "item-1",
		sessionId: "ses-1",
		turnId: "turn-1",
		seq: 1,
		revision: 1,
		createdAt: "2026-09-14T00:00:00.000Z",
		updatedAt: "2026-09-14T00:00:01.000Z",
		state: "completed",
		...overrides,
	}
}

describe("extractAutomationItem", () => {
	test("reads terminal assistantMessage text from Native item payload", () => {
		expect(
			extractAutomationItem(
				envelope({
					item: { type: "assistantMessage", text: "Actionable: yes\nDone." },
				}),
			),
		).toEqual({
			text: "Actionable: yes\nDone.",
			toolName: null,
		})
	})

	test("ignores streaming assistantMessage deltas", () => {
		expect(
			extractAutomationItem(
				envelope({
					state: "running",
					item: { type: "assistantMessage", text: "partial" },
				}),
			),
		).toEqual({ text: null, toolName: null })
	})

	test("reads toolName from toolCall and hostedToolCall payloads", () => {
		expect(
			extractAutomationItem(
				envelope({
					state: "running",
					item: { type: "toolCall", toolName: "bash", callId: "call-1" },
				}),
			),
		).toEqual({ text: null, toolName: "bash" })

		expect(
			extractAutomationItem(
				envelope({
					item: { type: "hostedToolCall", callId: "hosted-9" },
				}),
			),
		).toEqual({ text: null, toolName: "hosted-9" })
	})

	test("ignores userMessage and other item types", () => {
		expect(
			extractAutomationItem(
				envelope({
					item: {
						type: "userMessage",
						content: [{ type: "text", text: "hello" }],
					},
				}),
			),
		).toEqual({ text: null, toolName: null })
	})
})
