/**
 * Mention tracking system.
 *
 * Maintains a list of @-mentions alongside the textarea text.
 * When the text changes, mentions whose `@displayName` text is no longer present
 * are automatically removed.
 */
import type { MentionOption } from "./mention-popover"

// ============================================================
// Types
// ============================================================

export interface FileMention {
	type: "file"
	path: string
	displayName: string
	marker: string
}

export interface AgentMention {
	type: "agent"
	name: string
	displayName: string
}

export interface ReferenceMention {
	type: "reference"
	kind: "skill" | "mcp"
	name: string
	displayName: string
	marker: string
	path?: string
}

export type PromptMention = FileMention | AgentMention | ReferenceMention

// ============================================================
// Helpers
// ============================================================

/** Get the text marker for a mention (what appears in the textarea) */
export function getMentionMarker(mention: PromptMention): string {
	return mention.type === "agent" ? `@${mention.displayName}` : mention.marker
}

/** Get the unique key for a mention */
export function getMentionKey(mention: PromptMention): string {
	switch (mention.type) {
		case "file":
			return `file:${mention.path}`
		case "agent":
			return `agent:${mention.name}`
		case "reference":
			return `${mention.kind}:${mention.path ?? mention.name}`
	}
}

/**
 * Reconcile mentions with the current text.
 * Removes any mentions whose marker text is no longer present in the input.
 */
export function reconcileMentions(mentions: PromptMention[], text: string): PromptMention[] {
	return mentions.filter((m) => {
		const marker = getMentionMarker(m)
		return text.includes(marker)
	})
}

/**
 * Insert a mention into text at the trigger position.
 * Replaces `@query` text with `@displayName ` and returns the updated text + cursor position.
 */
export function insertMentionIntoText(
	text: string,
	cursorPosition: number,
	mention: PromptMention,
): { text: string; cursorPosition: number } {
	// Find the `@` trigger before the cursor
	const beforeCursor = text.slice(0, cursorPosition)
	const atMatch = beforeCursor.match(/@(\S*)$/)

	if (!atMatch || atMatch.index === undefined) {
		// Fallback: just append
		const marker = `${getMentionMarker(mention)} `
		return {
			text: text + marker,
			cursorPosition: text.length + marker.length,
		}
	}

	const atStart = atMatch.index
	const atEnd = cursorPosition
	const marker = `${getMentionMarker(mention)} `

	const newText = text.slice(0, atStart) + marker + text.slice(atEnd)
	const newCursor = atStart + marker.length

	return { text: newText, cursorPosition: newCursor }
}

/**
 * Create a FileMention from a file path.
 */
export function createFileMention(path: string, marker?: string): FileMention {
	// Display name is the filename (or full path for short paths)
	const parts = path.split("/")
	const fileName = parts[parts.length - 1] || path
	return {
		type: "file",
		path,
		displayName: path.length > 40 ? fileName : path,
		marker: marker ?? `@${path.length > 40 ? fileName : path}`,
	}
}

/**
 * Create an AgentMention from an agent name.
 */
export function createAgentMention(name: string): AgentMention {
	return {
		type: "agent",
		name,
		displayName: name,
	}
}

export function createReferenceMention(
	kind: "skill" | "mcp",
	name: string,
	insertText: string,
	path?: string,
): ReferenceMention {
	return {
		type: "reference",
		kind,
		name,
		displayName: name,
		marker: insertText,
		path,
	}
}

export function createMentionFromOption(option: MentionOption): PromptMention {
	switch (option.type) {
		case "agent":
			return createAgentMention(option.name)
		case "file":
			return createFileMention(option.path, option.insertText)
		case "skill":
		case "mcp":
			return createReferenceMention(
				option.type,
				option.name,
				option.insertText,
				option.mentionPath,
			)
	}
}
