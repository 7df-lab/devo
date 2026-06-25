/**
 * Onboarding state atoms.
 *
 * Tracks whether the first-run onboarding has been completed.
 * Persisted to localStorage so returning users skip onboarding.
 */

import { atomWithStorage } from "jotai/utils"

// ============================================================
// Types
// ============================================================

export interface OnboardingState {
	completed: boolean
	completedAt: string | null
	skippedSteps: string[]
	migrationPerformed: boolean
	/** Which provider(s) were migrated from (e.g. ["claude-code", "cursor"]). */
	migratedFrom: string[]
	devoVersion: string | null
	/** Number of AI providers connected during onboarding. */
	providersConnected: number
}

// ============================================================
// Atoms
// ============================================================

export const onboardingStateAtom = atomWithStorage<OnboardingState>("devo:onboarding", {
	completed: false,
	completedAt: null,
	skippedSteps: [],
	migrationPerformed: false,
	migratedFrom: [],
	devoVersion: null,
	providersConnected: 0,
})
