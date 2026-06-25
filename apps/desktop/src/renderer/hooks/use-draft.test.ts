import { describe, expect, test } from "bun:test"
import { NEW_CHAT_DRAFT_KEY, newChatDraftKey } from "./draft-keys"

describe("draft keys", () => {
	test("new chat drafts are scoped per project directory", () => {
		expect([
			newChatDraftKey(null),
			newChatDraftKey(""),
			newChatDraftKey("/repo/devo"),
			newChatDraftKey("/repo/devo-feat"),
		]).toEqual([
			NEW_CHAT_DRAFT_KEY,
			NEW_CHAT_DRAFT_KEY,
			"__new_chat__:/repo/devo",
			"__new_chat__:/repo/devo-feat",
		])
	})
})
