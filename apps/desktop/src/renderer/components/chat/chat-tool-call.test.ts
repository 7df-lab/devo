import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"
import { getToolDuration, shouldDefaultOpen } from "./chat-tool-call"

const elapsedHookSource = readFileSync(new URL("../../hooks/use-elapsed-time.ts", import.meta.url), "utf8")

describe("shouldDefaultOpen", () => {
	test("collapses tool output by default", () => {
		const tools = ["bash", "read", "edit", "write", "apply_patch", "glob", "grep", "list"]

		expect(Object.fromEntries(tools.map((tool) => [tool, shouldDefaultOpen(tool, "completed")]))).toEqual({
			bash: false,
			read: false,
			edit: false,
			write: false,
			apply_patch: false,
			glob: false,
			grep: false,
			list: false,
		})
	})

	test("keeps error output expanded", () => {
		expect({
			bash: shouldDefaultOpen("bash", "error"),
			read: shouldDefaultOpen("read", "error"),
			unknown: shouldDefaultOpen("unknown", "error"),
		}).toEqual({
			bash: true,
			read: true,
			unknown: true,
		})
	})
})

describe("getToolDuration", () => {
	test("uses SDK tool state start and end timestamps", () => {
		expect(
			getToolDuration({
				id: "tool-1",
				type: "tool",
				state: { status: "completed", time: { start: 1_000, end: 3_500 } },
			} as any),
		).toBe("2s")
	})

	test("clamps reversed timestamps instead of showing negative durations", () => {
		expect(
			getToolDuration({
				id: "tool-1",
				type: "tool",
				state: { status: "completed", time: { start: 3_500, end: 1_000 } },
			} as any),
		).toBe("0ms")
	})
})

describe("useToolElapsedTime source", () => {
	test("uses tool state time without renderer first-seen timestamps", () => {
		expect({
			usesStateStart: elapsedHookSource.includes("part.state.time"),
			usesFirstSeen: elapsedHookSource.includes("getPartFirstSeenAt"),
		}).toEqual({
			usesStateStart: true,
			usesFirstSeen: false,
		})
	})
})
