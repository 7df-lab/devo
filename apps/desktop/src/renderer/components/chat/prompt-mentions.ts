/**
 * Mention tracking system.
 *
 * Maintains a list of @-mentions (files and agents) alongside the textarea text.
 * When the text changes, mentions whose `@displayName` text is no longer present
 * are automatically removed.
 */

// ============================================================
// Types
// ============================================================

export interface FileMention {
	type: "file"
	path: string
	displayName: string
}

export interface AgentMention {
	type: "agent"
	name: string
	displayName: string
}

export type PromptMention = FileMention | AgentMention

// ============================================================
// Helpers
// ============================================================

/** Get the text marker for a mention (what appears in the textarea) */
export function getMentionMarker(mention: PromptMention): string {
	return `@${mention.displayName}`
}

/** Get the unique key for a mention */
export function getMentionKey(mention: PromptMention): string {
	return mention.type === "file" ? `file:${mention.path}` : `agent:${mention.name}`
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
export function createFileMention(path: string): FileMention {
	// Display name is the filename (or full path for short paths)
	const parts = path.split("/")
	const fileName = parts[parts.length - 1] || path
	return {
		type: "file",
		path,
		displayName: path.length > 40 ? fileName : path,
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
