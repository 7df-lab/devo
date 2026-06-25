/**
 * Derived atoms for tree-scoped interactive requests.
 *
 * When the agent spawns sub-agents (via the Task tool), permissions and
 * questions from those child sessions need to "bubble up" to the parent
 * session's UI so the user can respond without navigating away.
 *
 * Architecture:
 *   1. `childrenMapAtom` — shared map of parentID→childIDs[], recomputed only
 *      when the set of session IDs changes (not on every status update).
 *   2. `effectivePermissionFamily(sessionId)` — first pending permission in
 *      the session's subtree (self + all descendants).
 *   3. `effectiveQuestionFamily(sessionId)` — same for questions.
 *   4. `sessionBlockedFamily(sessionId)` — true if either of the above is set.
 */

import { atom } from "jotai"
import type { Getter } from "jotai"
import { atomFamily } from "jotai-family"
import type { PermissionRequest, QuestionRequest } from "../../lib/types"
import { sessionFamily, sessionIdsAtom } from "../sessions"
import { buildChildrenMap, findTreeRequest } from "../../lib/session-tree"

// ============================================================
// Shared children map — recomputes when session set changes
// ============================================================

/**
 * A map from parentID → list of childIDs for all currently-known sessions.
 *
 * This atom subscribes to `sessionIdsAtom` and iterates `sessionFamily` for
 * each ID to read the `parentID` field. It recomputes whenever sessions are
 * added or removed (i.e. when `sessionIdsAtom` changes), but NOT when a
 * session's permissions or status change — those don't affect the tree shape.
 */
export const childrenMapAtom = atom((get) => {
	const ids = get(sessionIdsAtom)
	const sessions = new Map<string, { parentID?: string }>()
	for (const id of ids) {
		const entry = get(sessionFamily(id))
		if (!entry) continue
		sessions.set(id, { parentID: entry.session.parentID })
	}
	return buildChildrenMap(sessions)
})

// ============================================================
// Helpers — build request maps from all sessions
// ============================================================

/**
 * Build a Map<sessionID, PermissionRequest[]> across all sessions.
 * Only sessions that actually have permissions are included.
 */
function buildPermissionMap(get: Getter, ids: Set<string>): Map<string, PermissionRequest[]> {
	const map = new Map<string, PermissionRequest[]>()
	for (const id of ids) {
		const entry = get(sessionFamily(id))
		if (!entry || entry.permissions.length === 0) continue
		map.set(id, entry.permissions)
	}
	return map
}

/**
 * Build a Map<sessionID, QuestionRequest[]> across all sessions.
 * Only sessions that actually have questions are included.
 */
function buildQuestionMap(get: Getter, ids: Set<string>): Map<string, QuestionRequest[]> {
	const map = new Map<string, QuestionRequest[]>()
	for (const id of ids) {
		const entry = get(sessionFamily(id))
		if (!entry || entry.questions.length === 0) continue
		map.set(id, entry.questions)
	}
	return map
}

// ============================================================
// Per-session effective request atoms
// ============================================================

/**
 * Returns the first pending PermissionRequest in the session's subtree —
 * i.e. from the session itself OR any of its descendant sub-agent sessions.
 *
 * The returned object includes both the request and the session ID it
 * belongs to, so the UI can show "from sub-agent" context and send the
 * response to the correct session.
 */
export const effectivePermissionFamily = atomFamily((sessionId: string) =>
	atom(
		(get): { request: PermissionRequest; sessionId: string } | undefined => {
			const ids = get(sessionIdsAtom)
			const childrenMap = get(childrenMapAtom)
			const permissionMap = buildPermissionMap(get, ids)
			return findTreeRequest(childrenMap, permissionMap, sessionId)
		},
	),
)

/**
 * Returns the first pending QuestionRequest in the session's subtree —
 * i.e. from the session itself OR any of its descendant sub-agent sessions.
 */
export const effectiveQuestionFamily = atomFamily((sessionId: string) =>
	atom(
		(get): { request: QuestionRequest; sessionId: string } | undefined => {
			const ids = get(sessionIdsAtom)
			const childrenMap = get(childrenMapAtom)
			const questionMap = buildQuestionMap(get, ids)
			return findTreeRequest(childrenMap, questionMap, sessionId)
		},
	),
)

/**
 * True if the session or any descendant has a pending permission or question.
 * Used to block the prompt input and show a "waiting" status in the sidebar.
 */
export const sessionBlockedFamily = atomFamily((sessionId: string) =>
	atom((get) => {
		return (
			get(effectivePermissionFamily(sessionId)) !== undefined ||
			get(effectiveQuestionFamily(sessionId)) !== undefined
		)
	}),
)

/**
 * Returns the IDs of all sessions that are descendants of the given session
 * (children, grandchildren, etc.), not including the session itself.
 *
 * Used for notification dismissal: when a user navigates to a session,
 * we also dismiss alerts for all its sub-agent sessions.
 */
export const sessionDescendantIdsFamily = atomFamily((sessionId: string) => {
	let prev: string[] = []
	return atom((get) => {
		const childrenMap = get(childrenMapAtom)
		const ids: string[] = []
		const queue = [sessionId]
		const seen = new Set([sessionId])
		for (const id of queue) {
			const children = childrenMap.get(id) ?? []
			for (const child of children) {
				if (seen.has(child)) continue
				seen.add(child)
				queue.push(child)
				ids.push(child)
			}
		}
		// Structural equality to avoid spurious re-renders
		if (ids.length === prev.length && ids.every((id, i) => id === prev[i])) return prev
		prev = ids
		return ids
	})
})
