import { describe, expect, test } from "bun:test"
import { getToolCategory } from "./tool-category"

describe("getToolCategory", () => {
	test("maps known tools to their category and falls back to other", () => {
		expect({
			read: getToolCategory("read"),
			glob: getToolCategory("glob"),
			grep: getToolCategory("grep"),
			list: getToolCategory("list"),
			edit: getToolCategory("edit"),
			write: getToolCategory("write"),
			applyPatch: getToolCategory("apply_patch"),
			bash: getToolCategory("bash"),
			task: getToolCategory("task"),
			todowrite: getToolCategory("todowrite"),
			todoread: getToolCategory("todoread"),
			question: getToolCategory("question"),
			requestUserInput: getToolCategory("request_user_input"),
			webfetch: getToolCategory("webfetch"),
			unknownMcp: getToolCategory("some_mcp_tool"),
		}).toEqual({
			read: "explore",
			glob: "explore",
			grep: "explore",
			list: "explore",
			edit: "edit",
			write: "edit",
			applyPatch: "edit",
			bash: "run",
			task: "delegate",
			todowrite: "plan",
			todoread: "plan",
			question: "ask",
			requestUserInput: "ask",
			webfetch: "fetch",
			unknownMcp: "other",
		})
	})
})
