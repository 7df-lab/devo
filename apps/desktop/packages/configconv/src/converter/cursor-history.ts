/**
 * Cursor chat history converter.
 *
 * Converts Cursor chat sessions (from state.vscdb) to the Devo session format.
 * Uses the same ConvertedSession/ConvertedMessage types as the Claude Code history
 * converter, so the writer layer can handle both identically.
 */

import { createHash } from "node:crypto"
import type { ConvertedMessage, ConvertedPart, ConvertedSession } from "../types/conversion-result"
import type {
	CursorHistoryMessage,
	CursorHistoryScanResult,
	CursorHistorySession,
} from "../types/cursor"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

/**
 * Convert Cursor chat history to Devo session format.
 */
export function convertCursorHistory(history: CursorHistoryScanResult): {
	sessions: ConvertedSession[]
	report: MigrationReport
} {
	const report = createEmptyReport()
	const sessions: ConvertedSession[] = []

	for (const session of history.sessions) {
		try {
			const converted = convertSession(session)
			if (converted) {
				sessions.push(converted)
				report.migrated.push({
					category: "history",
					source: `Cursor chat: ${session.title}`,
					target: converted.session.id,
					details: `${converted.messages.length} messages, mode: ${session.mode}${session.model ? `, model: ${session.model}` : ""}`,
				})
			}
		} catch (err) {
			report.errors.push(
				`Failed to convert Cursor chat ${session.composerId}: ${err instanceof Error ? err.message : String(err)}`,
			)
		}
	}

	report.migrated.push({
		category: "history",
		source: `${history.totalSessions} Cursor chat sessions, ${history.totalMessages} messages`,
		target: `${sessions.length} sessions converted`,
	})

	return { sessions, report }
}

/**
 * Convert a single Cursor chat session to Devo format.
 */
function convertSession(session: CursorHistorySession): ConvertedSession | null {
	const projectId = hashProjectPath(session.projectPath)
	const sessionId = `ses_cursor_${session.composerId.slice(0, 8)}`

	const messages = convertMessages(session.messages, sessionId)
	if (messages.length === 0) return null

	const title = session.title || generateTitle(session.messages)

	return {
		projectId,
		session: {
			id: sessionId,
			slug: slugify(title),
			version: "imported",
			projectID: projectId,
			directory: session.projectPath,
			title,
			time: {
				created: session.createdAt,
				updated: session.lastUpdatedAt,
			},
			summary: {
				additions: 0,
				deletions: 0,
				files: 0,
			},
		},
		messages,
	}
}

/**
 * Convert Cursor chat messages to Devo message format.
 */
function convertMessages(
	cursorMessages: CursorHistoryMessage[],
	sessionId: string,
): ConvertedMessage[] {
	const messages: ConvertedMessage[] = []
	let msgCounter = 0

	for (const msg of cursorMessages) {
		msgCounter++
		const msgId = `msg_${sessionId}_${msgCounter.toString().padStart(4, "0")}`
		const parts = convertParts(msg, msgId)

		if (parts.length === 0) continue

		messages.push({
			id: msgId,
			sessionID: sessionId,
			role: msg.role,
			time: {
				created: Date.now(),
				updated: Date.now(),
			},
			parts,
		})
	}

	return messages
}

/**
 * Convert a single Cursor message to Devo parts.
 */
function convertParts(msg: CursorHistoryMessage, msgId: string): ConvertedPart[] {
	const parts: ConvertedPart[] = []
	let partCounter = 0

	// Thinking blocks (before the main text, as reasoning)
	if (msg.thinkingBlocks) {
		for (const block of msg.thinkingBlocks) {
			if (block.thinking) {
				partCounter++
				parts.push({
					id: `part_${msgId}_${partCounter.toString().padStart(3, "0")}`,
					messageID: msgId,
					type: "reasoning",
					content: block.thinking,
				})
			}
		}
	}

	// Main text content
	if (msg.text) {
		partCounter++
		parts.push({
			id: `part_${msgId}_${partCounter.toString().padStart(3, "0")}`,
			messageID: msgId,
			type: "text",
			content: msg.text,
		})
	}

	// Tool results (for agent mode)
	if (msg.toolResults) {
		for (const tool of msg.toolResults) {
			partCounter++
			parts.push({
				id: `part_${msgId}_${partCounter.toString().padStart(3, "0")}`,
				messageID: msgId,
				type: "tool-result",
				content: tool.result ?? tool.error ?? JSON.stringify(tool),
			})
		}
	}

	return parts
}

/**
 * Generate a title from the first user message if no title exists.
 */
function generateTitle(messages: CursorHistoryMessage[]): string {
	const firstUser = messages.find((m) => m.role === "user")
	if (firstUser?.text) {
		const trimmed = firstUser.text.trim()
		if (trimmed.length <= 80) return trimmed
		return `${trimmed.slice(0, 77)}...`
	}
	return "Untitled chat"
}

function hashProjectPath(path: string): string {
	return createHash("sha256").update(path).digest("hex").slice(0, 16)
}

function slugify(text: string): string {
	return text
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-|-$/g, "")
		.slice(0, 50)
}
