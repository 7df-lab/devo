import type { MessageBoxSyncOptions } from "electron"
import type { SessionState } from "./notification-watcher"

export const CANCEL_QUIT_BUTTON_INDEX = 0
export const CONFIRM_QUIT_BUTTON_INDEX = 1

export function countWorkingRootSessions(sessions: ReadonlyMap<string, SessionState>): number {
	const workingRootSessionIds = new Set<string>()
	for (const [sessionId, session] of sessions.entries()) {
		if (session.status === "busy" || session.status === "retry") {
			let currentId = sessionId
			const visited = new Set<string>()

			while (!visited.has(currentId)) {
				visited.add(currentId)
				const parentID = sessions.get(currentId)?.parentID
				if (!parentID) break
				currentId = parentID
			}

			workingRootSessionIds.add(currentId)
		}
	}
	return workingRootSessionIds.size
}

export function shouldPromptBeforeQuit({
	sessions,
	quitConfirmed,
}: {
	sessions: ReadonlyMap<string, SessionState>
	quitConfirmed: boolean
}): boolean {
	return !quitConfirmed && countWorkingRootSessions(sessions) > 0
}

export function createWorkingSessionsQuitDialogOptions(workingSessionCount: number): MessageBoxSyncOptions {
	const plural = workingSessionCount === 1 ? "session is" : "sessions are"
	return {
		type: "warning",
		buttons: ["Cancel", "Quit Devo"],
		defaultId: CANCEL_QUIT_BUTTON_INDEX,
		cancelId: CANCEL_QUIT_BUTTON_INDEX,
		title: "Quit Devo?",
		message: `${workingSessionCount} ${plural} still working.`,
		detail: "Quitting Devo will stop the local server and interrupt active work.",
	}
}
