/** Key used for the global new-chat draft. */
export const NEW_CHAT_DRAFT_KEY = "__new_chat__"

export function newChatDraftKey(projectDirectory: string | null | undefined): string {
	return projectDirectory ? `${NEW_CHAT_DRAFT_KEY}:${projectDirectory}` : NEW_CHAT_DRAFT_KEY
}
