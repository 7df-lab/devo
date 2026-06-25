/**
 * Jotai atoms for the React Scan dev-tool toggle.
 *
 * React Scan is loaded via a synchronous script tag in index.html (outside
 * Vite's module graph) so it survives HMR. Toggling requires a page reload
 * because the DevTools hook must be installed before React initializes.
 */

import { atom } from "jotai"
import { atomWithStorage } from "jotai/utils"

const STORAGE_KEY = "devo:reactScan"

/**
 * Persisted toggle for React Scan.
 * Read by the inline script in index.html to decide whether to load react-scan.
 */
export const reactScanStorageAtom = atomWithStorage<boolean>(STORAGE_KEY, false)

/**
 * Read-only derived atom.
 */
export const isReactScanAtom = atom((get) => get(reactScanStorageAtom))

/**
 * Write-only toggle atom for the Command Palette.
 * Flips the persisted value and reloads the page so react-scan can
 * be loaded (or not) before React initializes.
 */
export const toggleReactScanAtom = atom(null, (get, set) => {
	const next = !get(reactScanStorageAtom)
	set(reactScanStorageAtom, next)

	// Small delay so Jotai's storage sync completes before reload
	setTimeout(() => {
		window.location.reload()
	}, 50)
})
