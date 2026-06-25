import { atom } from "jotai"
import { atomWithStorage } from "jotai/utils"

// ============================================================
// Mock mode state
// ============================================================

/**
 * Persisted toggle for demo/mock mode.
 * When true, the app uses static fixture data instead of connecting
 * to the Devo server. Used for screenshots and marketing.
 */
export const mockModeStorageAtom = atomWithStorage<boolean>("devo:mockMode", false)

/**
 * Check if ?mock=1 is present in the URL (works with both hash and regular URLs).
 */
function hasMockUrlParam(): boolean {
	if (typeof window === "undefined") return false
	const hash = window.location.hash
	const search = hash.includes("?") ? hash.slice(hash.indexOf("?")) : window.location.search
	const params = new URLSearchParams(search)
	return params.get("mock") === "1"
}

/**
 * Derived read atom: true if mock mode is active (via storage OR URL param).
 */
export const isMockModeAtom = atom((get) => {
	return get(mockModeStorageAtom) || hasMockUrlParam()
})

/**
 * Write-only toggle atom for the command palette.
 */
export const toggleMockModeAtom = atom(null, (get, set) => {
	set(mockModeStorageAtom, !get(mockModeStorageAtom))
})
