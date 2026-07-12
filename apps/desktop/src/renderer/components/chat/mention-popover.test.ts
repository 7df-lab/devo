import { describe, expect, test } from "bun:test"
import type { ReferenceSearchResult } from "@devo-ai/sdk/v2/client"
import { isMentionOptionDisabled, mapReferenceSearchResults } from "./mention-popover"
import { createMentionFromOption, insertMentionIntoText } from "./prompt-mentions"

describe("mention popover reference results", () => {
	test("preserves skill, MCP, and file results from the server", () => {
		const results: ReferenceSearchResult[] = [
			{
				kind: "skill",
				display_name: "openai-docs",
				description: "Use official OpenAI documentation",
				insert_text: "@openai-docs",
				mention_path: "skills/openai-docs/SKILL.md",
			},
			{
				kind: "mcp",
				display_name: "Docs",
				description: "Documentation server",
				insert_text: "@mcp:docs",
				mention_path: "mcp://server/docs",
				is_disabled: true,
				disabled_reason: "Server is disconnected",
			},
			{
				kind: "file",
				display_name: "src/main.rs",
				insert_text: "@main.rs",
				mention_path: "src/main.rs",
				file_path: "/workspace/src/main.rs",
			},
		]

		expect(mapReferenceSearchResults(results)).toEqual([
			{
				type: "skill",
				name: "openai-docs",
				display: "openai-docs",
				description: "Use official OpenAI documentation",
				insertText: "@openai-docs",
				mentionPath: "skills/openai-docs/SKILL.md",
				disabled: false,
				disabledReason: undefined,
			},
			{
				type: "mcp",
				name: "Docs",
				display: "Docs",
				description: "Documentation server",
				insertText: "@mcp:docs",
				mentionPath: "mcp://server/docs",
				disabled: true,
				disabledReason: "Server is disconnected",
			},
			{
				type: "file",
				path: "src/main.rs",
				display: "src/main.rs",
				insertText: "@main.rs",
				disabled: false,
				disabledReason: undefined,
			},
		])
	})

	test("inserts the exact server token for Skill and MCP selections", () => {
		const [skill, mcp] = mapReferenceSearchResults([
			{
				kind: "skill",
				display_name: "OpenAI Docs",
				insert_text: "@openai-docs",
				mention_path: "skills/openai-docs/SKILL.md",
			},
			{
				kind: "mcp",
				display_name: "Documentation",
				insert_text: "@mcp:docs",
				mention_path: "mcp://server/docs",
			},
		])

		expect([
			insertMentionIntoText("Ask @open", 9, createMentionFromOption(skill)),
			insertMentionIntoText("Use @doc", 8, createMentionFromOption(mcp)),
		]).toEqual([
			{ text: "Ask @openai-docs ", cursorPosition: 17 },
			{ text: "Use @mcp:docs ", cursorPosition: 14 },
		])
	})

	test("excludes references with a disabled reason from selection", () => {
		const [mcp] = mapReferenceSearchResults([
			{
				kind: "mcp",
				display_name: "Disconnected MCP",
				insert_text: "@mcp:disconnected",
				disabled_reason: "Server is disconnected",
			},
		])

		expect({ option: mcp, selectable: !isMentionOptionDisabled(mcp) }).toEqual({
			option: {
				type: "mcp",
				name: "Disconnected MCP",
				display: "Disconnected MCP",
				description: undefined,
				insertText: "@mcp:disconnected",
				mentionPath: undefined,
				disabled: true,
				disabledReason: "Server is disconnected",
			},
			selectable: false,
		})
	})
})
