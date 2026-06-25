/**
 * Tests for Cursor chat history converter.
 */
import { describe, expect, test } from "bun:test"
import { convertCursorHistory } from "../../src/converter/cursor-history"
import type {
	CursorHistoryMessage,
	CursorHistoryScanResult,
	CursorHistorySession,
} from "../../src/types/cursor"

function emptyHistory(): CursorHistoryScanResult {
	return {
		workspaces: [],
		sessions: [],
		totalSessions: 0,
		totalMessages: 0,
	}
}

function makeSession(overrides: Partial<CursorHistorySession> = {}): CursorHistorySession {
	return {
		composerId: "abc12345-1234-5678-abcd-123456789abc",
		title: "Test Chat Session",
		projectPath: "/Users/test/project",
		createdAt: 1700000000000,
		lastUpdatedAt: 1700001000000,
		mode: "agent",
		model: "claude-4-sonnet",
		messages: [
			{
				bubbleId: "bubble-001",
				role: "user",
				text: "Hello, please help me with this code",
			},
			{
				bubbleId: "bubble-002",
				role: "assistant",
				text: "Sure, I can help with that. Let me analyze the code.",
			},
		],
		...overrides,
	}
}

describe("convertCursorHistory", () => {
	test("returns empty result for empty history", () => {
		const result = convertCursorHistory(emptyHistory())

		expect(result.sessions).toHaveLength(0)
		expect(result.report.errors).toHaveLength(0)
		// Summary item is always added
		expect(result.report.migrated).toHaveLength(1)
	})

	test("converts a single session with messages", () => {
		const history = emptyHistory()
		const session = makeSession()
		history.sessions = [session]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)

		expect(result.sessions).toHaveLength(1)
		const converted = result.sessions[0]

		// Session metadata
		expect(converted.session.id).toStartWith("ses_cursor_abc12345")
		expect(converted.session.title).toBe("Test Chat Session")
		expect(converted.session.directory).toBe("/Users/test/project")
		expect(converted.session.version).toBe("imported")
		expect(converted.session.time.created).toBe(1700000000000)
		expect(converted.session.time.updated).toBe(1700001000000)

		// Messages
		expect(converted.messages).toHaveLength(2)
		expect(converted.messages[0].role).toBe("user")
		expect(converted.messages[0].parts).toHaveLength(1)
		expect(converted.messages[0].parts[0].type).toBe("text")
		expect(converted.messages[0].parts[0].content).toBe("Hello, please help me with this code")

		expect(converted.messages[1].role).toBe("assistant")
		expect(converted.messages[1].parts[0].content).toBe(
			"Sure, I can help with that. Let me analyze the code.",
		)
	})

	test("generates session slug from title", () => {
		const history = emptyHistory()
		history.sessions = [makeSession({ title: "Fix the Bug in my Code!" })]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)

		expect(result.sessions[0].session.slug).toBe("fix-the-bug-in-my-code")
	})

	test("generates project ID from path", () => {
		const history = emptyHistory()
		history.sessions = [
			makeSession({ projectPath: "/Users/test/project-a" }),
			makeSession({
				composerId: "def12345-1234-5678-abcd-123456789abc",
				projectPath: "/Users/test/project-b",
			}),
		]
		history.totalSessions = 2
		history.totalMessages = 4

		const result = convertCursorHistory(history)

		// Same project path should produce the same project ID
		expect(result.sessions[0].projectId).not.toBe(result.sessions[1].projectId)
		// Project ID is a hash
		expect(result.sessions[0].projectId).toHaveLength(16)
	})

	test("converts thinking blocks as reasoning parts", () => {
		const messages: CursorHistoryMessage[] = [
			{
				bubbleId: "b1",
				role: "user",
				text: "Explain this code",
			},
			{
				bubbleId: "b2",
				role: "assistant",
				text: "This code does X",
				thinkingBlocks: [{ thinking: "Let me analyze the structure..." }],
			},
		]

		const history = emptyHistory()
		history.sessions = [makeSession({ messages })]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		const assistantMsg = result.sessions[0].messages[1]

		// Thinking block comes first, then text
		expect(assistantMsg.parts).toHaveLength(2)
		expect(assistantMsg.parts[0].type).toBe("reasoning")
		expect(assistantMsg.parts[0].content).toBe("Let me analyze the structure...")
		expect(assistantMsg.parts[1].type).toBe("text")
		expect(assistantMsg.parts[1].content).toBe("This code does X")
	})

	test("converts tool results", () => {
		const messages: CursorHistoryMessage[] = [
			{
				bubbleId: "b1",
				role: "user",
				text: "Run the test",
			},
			{
				bubbleId: "b2",
				role: "assistant",
				text: "Running the test...",
				toolResults: [
					{ toolName: "bash", result: "All tests passed" },
					{ toolName: "read", result: "file contents here" },
				],
			},
		]

		const history = emptyHistory()
		history.sessions = [makeSession({ messages })]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		const assistantMsg = result.sessions[0].messages[1]

		// Text + 2 tool results
		expect(assistantMsg.parts).toHaveLength(3)
		expect(assistantMsg.parts[0].type).toBe("text")
		expect(assistantMsg.parts[1].type).toBe("tool-result")
		expect(assistantMsg.parts[1].content).toBe("All tests passed")
		expect(assistantMsg.parts[2].type).toBe("tool-result")
		expect(assistantMsg.parts[2].content).toBe("file contents here")
	})

	test("skips sessions with no messages", () => {
		const history = emptyHistory()
		history.sessions = [makeSession({ messages: [] })]
		history.totalSessions = 1
		history.totalMessages = 0

		const result = convertCursorHistory(history)
		expect(result.sessions).toHaveLength(0)
	})

	test("generates title from first user message when no title exists", () => {
		const history = emptyHistory()
		history.sessions = [
			makeSession({
				title: "",
				messages: [
					{ bubbleId: "b1", role: "user", text: "What is the meaning of life?" },
					{ bubbleId: "b2", role: "assistant", text: "42" },
				],
			}),
		]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		expect(result.sessions[0].session.title).toBe("What is the meaning of life?")
	})

	test("truncates long titles", () => {
		const longTitle =
			"This is a very long title that exceeds the maximum character limit and should be truncated to something reasonable for display purposes"

		const history = emptyHistory()
		history.sessions = [
			makeSession({
				title: "",
				messages: [
					{ bubbleId: "b1", role: "user", text: longTitle },
					{ bubbleId: "b2", role: "assistant", text: "ok" },
				],
			}),
		]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		expect(result.sessions[0].session.title.length).toBeLessThanOrEqual(80)
		expect(result.sessions[0].session.title).toEndWith("...")
	})

	test("generates migration report items", () => {
		const history = emptyHistory()
		history.sessions = [makeSession()]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)

		// One item per session + one summary item
		expect(result.report.migrated.length).toBeGreaterThanOrEqual(2)
		expect(result.report.migrated[0].category).toBe("history")
		expect(result.report.migrated[0].source).toContain("Cursor chat: Test Chat Session")
	})

	test("handles multiple sessions across different projects", () => {
		const history = emptyHistory()
		history.sessions = [
			makeSession({
				composerId: "aaa11111-1111-1111-1111-111111111111",
				projectPath: "/Users/test/project-a",
				title: "Session A",
			}),
			makeSession({
				composerId: "bbb22222-2222-2222-2222-222222222222",
				projectPath: "/Users/test/project-b",
				title: "Session B",
			}),
			makeSession({
				composerId: "ccc33333-3333-3333-3333-333333333333",
				projectPath: "/Users/test/project-a",
				title: "Session C",
			}),
		]
		history.totalSessions = 3
		history.totalMessages = 6

		const result = convertCursorHistory(history)

		expect(result.sessions).toHaveLength(3)

		// Sessions from same project should have same project ID
		expect(result.sessions[0].projectId).toBe(result.sessions[2].projectId)
		expect(result.sessions[0].projectId).not.toBe(result.sessions[1].projectId)
	})

	test("message IDs are sequential within a session", () => {
		const history = emptyHistory()
		history.sessions = [makeSession()]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		const messages = result.sessions[0].messages

		expect(messages[0].id).toContain("_0001")
		expect(messages[1].id).toContain("_0002")
	})

	test("part IDs reference their parent message", () => {
		const history = emptyHistory()
		history.sessions = [makeSession()]
		history.totalSessions = 1
		history.totalMessages = 2

		const result = convertCursorHistory(history)
		const msg = result.sessions[0].messages[0]
		const part = msg.parts[0]

		expect(part.messageID).toBe(msg.id)
		expect(part.id).toContain(msg.id)
	})
})
