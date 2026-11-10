/**
 * Request ownership follows the shared cache atom, not a mounted hook/session.
 * A new Summary invalidates older details even if their transport cannot cancel.
 * Weak keys let evicted cache atoms and their request bookkeeping be collected.
 */
type WorkspaceChangeRequests = {
	generation: number
	summaryPending: boolean
	detailsPending: Set<string>
	detailErrors: Map<string, string>
	sidesFetched: Set<string>
	requestedPaths: Set<string>
}

const requestsByCache = new WeakMap<object, WorkspaceChangeRequests>()

export function workspaceChangeRequests(cache: object): WorkspaceChangeRequests {
	let requests = requestsByCache.get(cache)
	if (!requests) {
		requests = {
			generation: 0,
			summaryPending: false,
			detailsPending: new Set(),
			detailErrors: new Map(),
			sidesFetched: new Set(),
			requestedPaths: new Set(),
		}
		requestsByCache.set(cache, requests)
	}
	return requests
}
