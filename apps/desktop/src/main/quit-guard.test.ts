import { describe, expect, test } from "bun:test"
import type { SessionState } from "./notification-watcher"
import {
	CANCEL_QUIT_BUTTON_INDEX,
	CONFIRM_QUIT_BUTTON_INDEX,
	countWorkingRootSessions,
	createWorkingSessionsQuitDialogOptions,
	shouldPromptBeforeQuit,
} from "./quit-guard"

function session(status: string, parentID?: string): SessionState {
	return {
		status,
		title: "Session",
		parentID,
	}
}

describe("desktop quit guard", () => {
	test("prompts when root sessions are busy or retrying", () => {
		const sessions = new Map<string, SessionState>([
			["busy-root", session("busy")],
			["retry-root", session("retry")],
			["idle-root", session("idle")],
			["busy-child", session("busy", "busy-root")],
		])

		expect(countWorkingRootSessions(sessions)).toBe(2)
		expect(shouldPromptBeforeQuit({ sessions, quitConfirmed: false })).toBe(true)
	})

	test("does not prompt for idle sessions", () => {
		const sessions = new Map<string, SessionState>([["idle-root", session("idle")]])

		expect(countWorkingRootSessions(sessions)).toBe(0)
		expect(shouldPromptBeforeQuit({ sessions, quitConfirmed: false })).toBe(false)
	})

	test("counts sub-agent work against its root session without double counting", () => {
		const sessions = new Map<string, SessionState>([
			["idle-root", session("idle")],
			["busy-child", session("busy", "idle-root")],
			["retry-grandchild", session("retry", "busy-child")],
		])

		expect(countWorkingRootSessions(sessions)).toBe(1)
		expect(shouldPromptBeforeQuit({ sessions, quitConfirmed: false })).toBe(true)
	})

	test("does not prompt again after the quit was confirmed", () => {
		const sessions = new Map<string, SessionState>([["busy-root", session("busy")]])

		expect(shouldPromptBeforeQuit({ sessions, quitConfirmed: true })).toBe(false)
	})

	test("builds the native dialog options for active work", () => {
		expect(createWorkingSessionsQuitDialogOptions(2)).toEqual({
			type: "warning",
			buttons: ["Cancel", "Quit Devo"],
			defaultId: CANCEL_QUIT_BUTTON_INDEX,
			cancelId: CANCEL_QUIT_BUTTON_INDEX,
			title: "Quit Devo?",
			message: "2 sessions are still working.",
			detail: "Quitting Devo will stop the local server and interrupt active work.",
		})
		expect(CONFIRM_QUIT_BUTTON_INDEX).toBe(1)
	})
})
