/**
 * React hooks for accessing automation state.
 */

import { useAtomValue } from "jotai"
import {
	activeAutomationCountAtom,
	automationRunsAtom,
	automationsAtom,
	pendingRunCountAtom,
	unreadRunCountAtom,
} from "../atoms/automations"

export function useAutomations() {
	return useAtomValue(automationsAtom)
}

export function useAutomationRuns() {
	return useAtomValue(automationRunsAtom)
}

export function useActiveAutomationCount() {
	return useAtomValue(activeAutomationCountAtom)
}

export function usePendingRunCount() {
	return useAtomValue(pendingRunCountAtom)
}

export function useUnreadRunCount() {
	return useAtomValue(unreadRunCountAtom)
}
