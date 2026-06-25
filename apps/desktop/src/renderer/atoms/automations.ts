/**
 * Jotai atoms for automation state management.
 */

import { atom } from "jotai"
import type { Automation, AutomationRun } from "../../preload/api"

/** List of all automations. */
export const automationsAtom = atom<Automation[]>([])

/** List of automation runs (inbox items). */
export const automationRunsAtom = atom<AutomationRun[]>([])

/** Write-only atom to set automations from a fetch. */
export const setAutomationsAtom = atom(null, (_get, set, automations: Automation[]) => {
	set(automationsAtom, automations)
})

/** Write-only atom to set automation runs from a fetch. */
export const setAutomationRunsAtom = atom(null, (_get, set, runs: AutomationRun[]) => {
	set(automationRunsAtom, runs)
})

/** Derived: count of active (non-archived/non-paused) automations. */
export const activeAutomationCountAtom = atom((get) => {
	return get(automationsAtom).filter((a) => a.status === "active").length
})

/** Derived: count of pending review runs (for badge). */
export const pendingRunCountAtom = atom((get) => {
	return get(automationRunsAtom).filter(
		(r) => r.status === "pending_review" || r.status === "running",
	).length
})

/** Derived: count of unread runs (readAt is null AND status is pending_review). */
export const unreadRunCountAtom = atom((get) => {
	return get(automationRunsAtom).filter((r) => r.readAt === null && r.status === "pending_review")
		.length
})

/** Write-only atom to optimistically mark a run as read locally. */
export const markRunReadLocalAtom = atom(null, (get, set, runId: string) => {
	const runs = get(automationRunsAtom)
	const updated = runs.map((r) => (r.id === runId ? { ...r, readAt: Date.now() } : r))
	set(automationRunsAtom, updated)
})

/** Write-only atom to optimistically archive a run locally. */
export const archiveRunLocalAtom = atom(null, (get, set, runId: string) => {
	const runs = get(automationRunsAtom)
	const updated = runs.map((r) => (r.id === runId ? { ...r, status: "archived" as const } : r))
	set(automationRunsAtom, updated)
})
