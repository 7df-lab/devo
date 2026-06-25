/**
 * Feature flags -- persisted toggles for experimental features.
 *
 * Each flag is an independent atomWithStorage so they survive reloads.
 * Toggle via the command palette (Cmd+K > "Enable/Disable ...").
 */

import { atom } from "jotai"
import { atomWithStorage } from "jotai/utils"

// ============================================================
// Automations
// ============================================================

/**
 * Whether the Automations feature is enabled.
 * Enabled by default -- users can disable it via the command palette.
 */
export const automationsEnabledAtom = atomWithStorage<boolean>("devo:automationsEnabled", true)

/** Write-only toggle for the command palette. */
export const toggleAutomationsAtom = atom(null, (get, set) => {
	set(automationsEnabledAtom, !get(automationsEnabledAtom))
})
