import { atom } from "jotai"
import { sessionFamily, sessionIdsAtom } from "../sessions"

/**
 * Derived atom: true if any session has pending permissions or questions.
 * Used by the waiting indicator to update the document title.
 */
export const hasWaitingAtom = atom((get) => {
	const sessionIds = get(sessionIdsAtom)
	for (const id of sessionIds) {
		const entry = get(sessionFamily(id))
		if (!entry) continue
		if (entry.permissions.length > 0 || entry.questions.length > 0) return true
	}
	return false
})

/**
 * Derived atom: total count of pending permissions + questions across all sessions.
 * Used by the dock badge to show how many items need attention.
 */
export const pendingCountAtom = atom((get) => {
	const sessionIds = get(sessionIdsAtom)
	let count = 0
	for (const id of sessionIds) {
		const entry = get(sessionFamily(id))
		if (!entry) continue
		count += entry.permissions.length + entry.questions.length
	}
	return count
})
