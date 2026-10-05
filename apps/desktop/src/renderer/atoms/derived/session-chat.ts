import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import {
	assistantOrReasoningText,
	isUserMessageItem,
	nativeItemType,
	userMessageText,
} from "@devo-ai/sdk/v2/client"
import type { SessionItem } from "../messages"

/** A transcript row — Native ItemEnvelope directly (no Part dual). */
export interface ChatMessageEntry {
	info: SessionItem
}

/**
 * A "turn" groups a userMessage with following items that share turnId
 * (or sequential assistants after the user when turnId is absent).
 */
export interface ChatTurn {
	id: string
	turnId?: string
	userMessage: ChatMessageEntry
	assistantMessages: ChatMessageEntry[]
}

function itemFingerprint(entry: ChatMessageEntry): string {
	const info = entry.info
	const type = nativeItemType(info)
	const textLen =
		type === "userMessage"
			? userMessageText(info).length
			: type === "assistantMessage" || type === "reasoning"
				? assistantOrReasoningText(info).length
				: JSON.stringify(info.item).length
	return `${info.id}:${info.revision}:${info.state}:${type}:${textLen}`
}

function turnFingerprint(turn: ChatTurn): string {
	const assistantFps = turn.assistantMessages.map(itemFingerprint).join("|")
	return `${turn.turnId ?? ""}:${itemFingerprint(turn.userMessage)}>${assistantFps}`
}

export function groupIntoTurns(entries: ChatMessageEntry[], prevTurns: ChatTurn[]): ChatTurn[] {
	const prevMap = new Map<string, ChatTurn>()
	for (const t of prevTurns) {
		prevMap.set(turnFingerprint(t), t)
	}

	const turns: ChatTurn[] = []
	let currentUser: ChatMessageEntry | null = null
	let currentAssistantMessages: ChatMessageEntry[] = []
	let currentTurnId: string | undefined

	const flushTurn = () => {
		if (!currentUser) return
		const newTurn: ChatTurn = {
			id: currentUser.info.id,
			turnId: currentTurnId || currentUser.info.turnId || undefined,
			userMessage: currentUser,
			assistantMessages: currentAssistantMessages,
		}
		const fp = turnFingerprint(newTurn)
		turns.push(prevMap.get(fp) ?? newTurn)
	}

	for (const entry of entries) {
		if (isUserMessageItem(entry.info)) {
			flushTurn()
			currentUser = entry
			currentAssistantMessages = []
			currentTurnId = entry.info.turnId || undefined
			continue
		}
		if (!currentUser) continue
		const entryTurn = entry.info.turnId || undefined
		if (currentTurnId && entryTurn && entryTurn !== currentTurnId) {
			flushTurn()
			currentUser = null
			currentAssistantMessages = []
			currentTurnId = undefined
			continue
		}
		currentAssistantMessages.push(entry)
	}
	flushTurn()

	return turns
}

/** Build ChatMessageEntry[] from Native session items (envelopes carry live text). */
export function mergeSessionItems(items: SessionItem[]): ChatMessageEntry[] {
	return items.map((info) => ({ info }))
}

export function itemDisplayText(envelope: NativeItemEnvelope): string {
	const type = nativeItemType(envelope)
	if (type === "userMessage") return userMessageText(envelope)
	if (type === "assistantMessage" || type === "reasoning") return assistantOrReasoningText(envelope)
	if (type === "commandExecution") {
		return String(envelope.item.command ?? envelope.item.output ?? "")
	}
	if (type === "toolCall" || type === "hostedToolCall") {
		return String(envelope.item.toolName ?? envelope.item.callId ?? type)
	}
	if (type === "contextCompaction") {
		return String(envelope.item.summary ?? "Context compaction")
	}
	if (type === "plan") {
		const entries = Array.isArray(envelope.item.entries) ? envelope.item.entries : []
		return entries
			.map((e) => {
				const row = e && typeof e === "object" ? (e as Record<string, unknown>) : {}
				return String(row.step ?? row.content ?? "")
			})
			.filter(Boolean)
			.join("\n")
	}
	return type
}
