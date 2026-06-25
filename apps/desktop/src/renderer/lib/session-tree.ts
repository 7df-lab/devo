/**
 * Pure utilities for walking the session parent→child tree to surface
 * interactive requests (permissions, questions) from sub-agent sessions
 * up to their parent session's UI.
 *
 * These are plain functions with no React or Jotai dependencies so they
 * can be called from both atoms and pure test code.
 */

// ============================================================
// Tree walk
// ============================================================

/**
 * Build a map from parentID → list of childIDs using the provided session entries.
 */
export function buildChildrenMap(
	sessions: Map<string, { parentID?: string }>,
): Map<string, string[]> {
	const map = new Map<string, string[]>()
	for (const [id, session] of sessions) {
		if (!session.parentID) continue
		const existing = map.get(session.parentID)
		if (existing) {
			existing.push(id)
		} else {
			map.set(session.parentID, [id])
		}
	}
	return map
}

/**
 * BFS from `rootSessionId` using the children map, returning all descendant
 * session IDs (including the root itself).
 */
export function getSessionDescendants(
	childrenMap: Map<string, string[]>,
	rootSessionId: string,
): string[] {
	const seen = new Set([rootSessionId])
	const ids = [rootSessionId]

	for (const id of ids) {
		const children = childrenMap.get(id)
		if (!children) continue
		for (const child of children) {
			if (seen.has(child)) continue
			seen.add(child)
			ids.push(child)
		}
	}

	return ids
}

/**
 * Walk the full session tree rooted at `rootSessionId` and return the first
 * item from `requests` (keyed by sessionID) that satisfies `include`.
 *
 * The walk is breadth-first so the root session's own requests are checked
 * first, then its direct children, etc.
 *
 * @param childrenMap  Pre-built parentID→childIDs map (from buildChildrenMap)
 * @param requests     Map from sessionID to list of pending requests of type T
 * @param rootSessionId The session whose subtree to search
 * @param include      Optional predicate to filter which requests to consider
 */
export function findTreeRequest<T>(
	childrenMap: Map<string, string[]>,
	requests: Map<string, T[]>,
	rootSessionId: string,
	include: (item: T) => boolean = () => true,
): { request: T; sessionId: string } | undefined {
	const ids = getSessionDescendants(childrenMap, rootSessionId)

	for (const id of ids) {
		const list = requests.get(id)
		if (!list) continue
		const found = list.find(include)
		if (found !== undefined) return { request: found, sessionId: id }
	}

	return undefined
}

// ============================================================
// Convenience helpers
// ============================================================

/**
 * Returns true if the root session or any descendant has at least one item
 * in `requests` satisfying `include`.
 */
export function hasTreeRequest<T>(
	childrenMap: Map<string, string[]>,
	requests: Map<string, T[]>,
	rootSessionId: string,
	include?: (item: T) => boolean,
): boolean {
	return findTreeRequest(childrenMap, requests, rootSessionId, include) !== undefined
}
