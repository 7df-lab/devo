import { useAtomValue, useSetAtom } from "jotai"
import { useEffect } from "react"
import type { WindowChromeTier } from "../../preload/api"
import { chromeTierAtom, isTransparentAtom, opaqueWindowsAtom } from "../atoms/preferences"

/**
 * Detect whether we're running inside Electron (preload injects `window.devo`).
 */
function isElectron(): boolean {
	return typeof window !== "undefined" && "devo" in window
}

/** All glass-related CSS classes that we toggle on <html>. */
const GLASS_CLASSES = ["electron-transparent", "electron-vibrancy", "electron-opaque"] as const

/**
 * Subscribes to the chrome tier from the main process and syncs it to the Jotai store.
 * Manages three mutually exclusive CSS classes on <html>:
 *
 *   - `electron-transparent` — Liquid Glass (macOS 26+) or Windows transparency.
 *     CSS adds semi-transparent backgrounds + backdrop-blur on floating panels.
 *   - `electron-vibrancy` — Tier 2: Legacy vibrancy (older macOS). CSS adds
 *     semi-transparent backgrounds but NO backdrop-blur (native handles it).
 *   - `electron-opaque` — Tier 3: Solid backgrounds (Linux, user pref, or
 *     glass-disabled theme). Identical to current/stock look.
 *
 * In browser-mode dev (no Electron), no class is applied — globals.css glass
 * rules use `:root.electron-transparent` / `:root.electron-vibrancy` selectors,
 * so they are inert when no class is present.
 *
 * Call this once in the root layout.
 */
export function useChromeTier() {
	const setChromeTier = useSetAtom(chromeTierAtom)
	const chromeTier = useAtomValue(chromeTierAtom)
	const isTransparent = useAtomValue(isTransparentAtom)
	const isOpaque = useAtomValue(opaqueWindowsAtom)

	// Pull the chrome tier immediately on mount (avoids race with push event)
	useEffect(() => {
		if (!isElectron()) return

		window.devo.getChromeTier().then((tier) => {
			setChromeTier(tier)
		})
	}, [setChromeTier])

	// Also listen for the push event (backup / future tier changes)
	useEffect(() => {
		if (!isElectron()) return

		const unsubscribe = window.devo.onChromeTier((tier: string) => {
			setChromeTier(tier as WindowChromeTier)
		})

		return unsubscribe
	}, [setChromeTier])

	// Sync CSS class on <html> — three mutually exclusive states
	useEffect(() => {
		const root = document.documentElement

		// Remove all glass classes first
		for (const cls of GLASS_CLASSES) {
			root.classList.remove(cls)
		}

		// In browser mode, don't add any class — all glass CSS is inert
		if (!isElectron()) return

		if (isOpaque || chromeTier === "opaque") {
			root.classList.add("electron-opaque")
		} else if (chromeTier === "liquid-glass" || chromeTier === "transparent") {
			root.classList.add("electron-transparent")
		} else if (chromeTier === "vibrancy") {
			root.classList.add("electron-vibrancy")
		} else {
			root.classList.add("electron-opaque")
		}
	}, [chromeTier, isOpaque])

	// Set data-platform on <html> so CSS can apply platform-specific styles
	// (e.g. disabling hover states on macOS to match native sidebar behavior)
	useEffect(() => {
		if (!isElectron()) return
		document.documentElement.dataset.platform = window.devo.platform
	}, [])

	// Track native window focus for Windows chrome styling.
	useEffect(() => {
		if (!isElectron()) return

		const root = document.documentElement
		const setWindowFocus = (focused: boolean) => {
			root.dataset.windowFocused = focused ? "true" : "false"
		}
		const handleFocus = () => setWindowFocus(true)
		const handleBlur = () => setWindowFocus(false)

		setWindowFocus(document.hasFocus())
		window.addEventListener("focus", handleFocus)
		window.addEventListener("blur", handleBlur)

		return () => {
			window.removeEventListener("focus", handleFocus)
			window.removeEventListener("blur", handleBlur)
			delete root.dataset.windowFocused
		}
	}, [])

	return { isTransparent, isOpaque, chromeTier }
}

/**
 * Read-only hook to get whether the window is currently transparent.
 */
export function useIsTransparent(): boolean {
	return useAtomValue(isTransparentAtom)
}
