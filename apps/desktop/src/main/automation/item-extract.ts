/**
 * Pure helpers for reading automation-relevant fields from Native ItemEnvelope
 * payloads (`item.updated` events). Kept free of Electron / SDK client imports
 * so unit tests can run under bun without the main-process runtime.
 */

import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { assistantOrReasoningText, nativeItemType } from "@devo-ai/sdk/v2/client"

/** Text / tool fields pulled from a Native `item.updated` envelope payload. */
export type AutomationItemExtract = {
	text: string | null
	toolName: string | null
}

function isTerminalItemState(state: string): boolean {
	return state === "completed" || state === "failed" || state === "interrupted"
}

/**
 * Extracts assistant text and tool names from a Native ItemEnvelope `item` payload.
 * Streaming (non-terminal) assistant deltas are ignored so partial text is not duplicated.
 */
export function extractAutomationItem(envelope: NativeItemEnvelope): AutomationItemExtract {
	const type = nativeItemType(envelope)
	if (type === "assistantMessage") {
		if (!isTerminalItemState(envelope.state)) {
			return { text: null, toolName: null }
		}
		const text = assistantOrReasoningText(envelope)
		return { text: text.length > 0 ? text : null, toolName: null }
	}
	if (type === "toolCall" || type === "hostedToolCall") {
		const toolName = String(envelope.item.toolName ?? envelope.item.callId ?? "").trim()
		return { text: null, toolName: toolName.length > 0 ? toolName : null }
	}
	return { text: null, toolName: null }
}
