import { describe, expect, test } from "bun:test"
import type { ChatMessageEntry } from "../../hooks/use-session-chat"
import { activityRowStatus, activitySummary, buildActivityRows, groupTurnSegments } from "./turn-activity-model"

function entry(id: string, item: Record<string, unknown>, state = "completed"): ChatMessageEntry {
	return { info: {
		id, sessionId: "s1", turnId: "t1", seq: 1, revision: 1, state,
		createdAt: "2026-10-08T09:00:00Z", updatedAt: "2026-10-08T09:00:02Z", item,
	} }
}

const thought = entry("thought", { type: "reasoning", text: "Let me calculate." })
const call = entry("call", { type: "toolCall", callId: "c1", toolName: "ipython", input: { code: "print(42)" } })
const result = entry("result", { type: "toolResult", callId: "c1", output: "42", isError: false })
const answer = entry("answer", { type: "assistantMessage", text: "42" })

describe("turn activity", () => {
	test("keeps source and output in one row", () => {
		expect(buildActivityRows([thought, call, result])).toEqual([
			{ entry: thought }, { entry: call, result },
		])
	})
	test("does not discard an unmatched tool result", () => {
		const orphan = entry("orphan", { type: "toolResult", callId: "c2", output: "visible output" })
		expect(buildActivityRows([call, orphan])).toEqual([{ entry: call }, { entry: orphan }])
	})
	test("collects all activity in one disclosure across assistant commentary", () => {
		const notice = entry("notice", { type: "warning", code: "ipythonKernelStateFresh" })
		const commentary = entry("commentary", { type: "assistantMessage", text: "I'll run Python." })
		expect(groupTurnSegments([notice, thought, commentary, call, result, answer])).toEqual([
			{ kind: "activity", id: thought.info.id, entries: [thought, call, result] },
			{ kind: "message", id: commentary.info.id, entry: commentary },
			{ kind: "message", id: answer.info.id, entry: answer },
		])
	})
	test("summarizes completion without item counts or repeated Done labels", () => {
		expect(activitySummary([thought, call, result], false)).toBe("Worked for 2s")
		expect(activitySummary([thought], false)).toBe("Thought for 2s")
	})
	test("reflects the active phase", () => {
		const runningCall = entry("call", call.info.item, "running")
		const runningThought = entry("thought", thought.info.item, "running")
		expect([
			activitySummary([], true), activitySummary([runningThought], true),
			activitySummary([thought, runningCall], true), activitySummary([call, result], true),
		]).toEqual(["Working…", "Thinking…", "Running Python…", "Working…"])
	})
	test("distinguishes failure, deliberate stop, and normal completion", () => {
		const failure = entry("failed", { ...result.info.item, isError: true, output: "ValueError" })
		const stopped = entry("stopped", { ...result.info.item, isError: true, output: "tool execution was interrupted after 11271ms" })
		expect([
			activityRowStatus({ entry: call, result }),
			activityRowStatus({ entry: call, result: failure }),
			activityRowStatus({ entry: call, result: stopped }),
		]).toEqual([undefined, "Failed", "Stopped"])
	})
})
