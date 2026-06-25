/**
 * Session history converter (opt-in, P2 priority).
 *
 * Converts Claude Code JSONL session transcripts to Devo's JSON storage format.
 */

import { createHash } from "node:crypto"
import type {
	ClaudeContentBlock,
	ClaudeSessionApiMessage,
	ClaudeSessionLine,
} from "../types/claude-code"
import type {
	ConvertedMessage,
	ConvertedPart,
	ConvertedPromptEntry,
	ConvertedSession,
} from "../types/conversion-result"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import type { HistoryScanResult } from "../types/scan-result"
import { readJsonl } from "../utils/fs"

/**
 * Convert Claude Code session history to Devo format.
 */
export async function convertHistory(history: HistoryScanResult): Promise<{
	sessions: ConvertedSession[]
	promptHistory: ConvertedPromptEntry[]
	report: MigrationReport
}> {
	const report = createEmptyReport()
	const sessions: ConvertedSession[] = []

	for (const projectIndex of history.sessionIndices) {
		const projectId = hashProjectPath(projectIndex.projectPath)

		for (const entry of projectIndex.index.entries) {
			try {
				const lines = await readJsonl<ClaudeSessionLine>(entry.fullPath)
				// Filter to only user/assistant message lines
				const messageLines = lines.filter(
					(l) => (l.type === "user" || l.type === "assistant") && l.message,
				)
				if (messageLines.length === 0) continue

				const sessionId = `ses_imported_${entry.sessionId.slice(0, 8)}`
				const slug = slugify(entry.summary ?? entry.firstPrompt ?? "imported-session")

				const convertedMessages = convertSessionLines(messageLines, sessionId)

				sessions.push({
					projectId,
					session: {
						id: sessionId,
						slug,
						version: "imported",
						projectID: projectId,
						directory: projectIndex.projectPath,
						title: entry.summary ?? entry.firstPrompt ?? "Imported session",
						time: {
							created: entry.created ? new Date(entry.created).getTime() : Date.now(),
							updated: entry.modified ? new Date(entry.modified).getTime() : Date.now(),
						},
						summary: {
							additions: 0,
							deletions: 0,
							files: 0,
						},
					},
					messages: convertedMessages,
				})

				report.migrated.push({
					category: "history",
					source: entry.fullPath,
					target: sessionId,
					details: `${convertedMessages.length} messages, branch: ${entry.gitBranch ?? "unknown"}`,
				})
			} catch (err) {
				report.errors.push(
					`Failed to convert session ${entry.sessionId}: ${err instanceof Error ? err.message : String(err)}`,
				)
			}
		}
	}

	// Convert prompt history
	const promptHistory: ConvertedPromptEntry[] = (history.promptHistory ?? []).map((entry) => ({
		input: entry.display,
		parts: [],
		mode: "normal",
	}))

	if (promptHistory.length > 0) {
		report.migrated.push({
			category: "history",
			source: `${promptHistory.length} prompt history entries`,
			target: "prompt-history.jsonl",
		})
	}

	report.migrated.push({
		category: "history",
		source: `${history.totalSessions} sessions, ${history.totalMessages} messages`,
		target: `${sessions.length} sessions converted`,
	})

	return { sessions, promptHistory, report }
}

/**
 * Convert Claude Code session lines to Devo message format.
 *
 * Each line has `type: "user"|"assistant"` at the top level and
 * the actual Anthropic API message nested under `message`.
 * Assistant messages may contain content blocks (text, thinking, tool_use, tool_result).
 */
function convertSessionLines(lines: ClaudeSessionLine[], sessionId: string): ConvertedMessage[] {
	const messages: ConvertedMessage[] = []
	let msgCounter = 0

	for (const line of lines) {
		const apiMsg = line.message
		if (!apiMsg) continue

		msgCounter++
		const msgId = `msg_${sessionId}_${msgCounter.toString().padStart(4, "0")}`

		if (line.type === "user") {
			const content = extractTextContent(apiMsg)
			messages.push({
				id: msgId,
				sessionID: sessionId,
				role: "user",
				time: {
					created: Date.now(),
					updated: Date.now(),
				},
				parts: [
					{
						id: `part_${msgId}_001`,
						messageID: msgId,
						type: "text",
						content,
					},
				],
			})
		} else if (line.type === "assistant") {
			const parts = extractAssistantParts(apiMsg, msgId)
			messages.push({
				id: msgId,
				sessionID: sessionId,
				role: "assistant",
				time: {
					created: Date.now(),
					updated: Date.now(),
				},
				parts,
			})
		}
	}

	return messages
}

/**
 * Extract plain text content from an API message.
 */
function extractTextContent(apiMsg: ClaudeSessionApiMessage): string {
	if (typeof apiMsg.content === "string") return apiMsg.content

	if (Array.isArray(apiMsg.content)) {
		return apiMsg.content
			.filter((b): b is ClaudeContentBlock => typeof b === "object" && b !== null)
			.map((b) => {
				if (b.type === "text") return b.text ?? ""
				return ""
			})
			.filter(Boolean)
			.join("\n")
	}

	return ""
}

/**
 * Extract parts from an assistant API message (text, tool_use, thinking blocks).
 */
function extractAssistantParts(apiMsg: ClaudeSessionApiMessage, msgId: string): ConvertedPart[] {
	const parts: ConvertedPart[] = []
	let partCounter = 0

	if (typeof apiMsg.content === "string") {
		partCounter++
		parts.push({
			id: `part_${msgId}_${partCounter.toString().padStart(3, "0")}`,
			messageID: msgId,
			type: "text",
			content: apiMsg.content,
		})
		return parts
	}

	if (!Array.isArray(apiMsg.content)) return parts

	for (const block of apiMsg.content) {
		if (typeof block !== "object" || block === null) continue
		const b = block as ClaudeContentBlock
		partCounter++
		const partId = `part_${msgId}_${partCounter.toString().padStart(3, "0")}`

		switch (b.type) {
			case "text":
				parts.push({
					id: partId,
					messageID: msgId,
					type: "text",
					content: b.text ?? "",
				})
				break
			case "thinking":
				parts.push({
					id: partId,
					messageID: msgId,
					type: "reasoning",
					content: b.thinking ?? "",
				})
				break
			case "tool_use":
				parts.push({
					id: partId,
					messageID: msgId,
					type: "tool-invocation",
					content: JSON.stringify({
						name: b.name,
						input: b.input,
						toolCallId: b.id,
					}),
				})
				break
			case "tool_result":
				parts.push({
					id: partId,
					messageID: msgId,
					type: "tool-result",
					content: typeof b.content === "string" ? b.content : JSON.stringify(b.content ?? ""),
				})
				break
		}
	}

	return parts
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
