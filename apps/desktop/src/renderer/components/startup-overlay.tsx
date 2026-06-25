/**
 * Full-screen overlay shown during initial app startup.
 *
 * Two-layer splash architecture:
 * 1. The HTML-level splash (`#splash` in index.html) renders instantly before
 *    any JS loads, blocking interaction with half-loaded UI.
 * 2. This React component mounts and immediately hides the HTML splash,
 *    taking over seamlessly. Uses `bg-background` so the liquid glass /
 *    vibrancy of the native window chrome shows through (same approach as
 *    the onboarding overlay).
 *
 * Fades out once discovery reaches the "ready" phase, then unmounts after
 * the CSS transition completes.
 */

import { useAtomValue } from "jotai"
import { useEffect, useRef, useState } from "react"
import { discoveryPhaseAtom } from "../atoms/discovery"
import { DevoSplashBrand } from "./devo-splash-brand"

// ============================================================
// Constants
// ============================================================

/** Duration of the fade-out transition (ms). Must match the CSS transition. */
const FADE_DURATION_MS = 400

// ============================================================
// Component
// ============================================================

/** Overlay lifecycle: visible -> fading -> unmounted (or skipped entirely). */
type OverlayState = "visible" | "fading" | "unmounted"

export function StartupOverlay() {
	const phase = useAtomValue(discoveryPhaseAtom)

	// If discovery is already complete on first render (e.g., eager server
	// finished before React mounted, or Vite HMR reload while server is running),
	// skip the overlay entirely.
	const initialPhaseRef = useRef(phase)
	const [state, setState] = useState<OverlayState>(
		initialPhaseRef.current === "ready" ? "unmounted" : "visible",
	)

	// On mount, remove the HTML-level splash (seamless handoff).
	// If skipping the overlay, fade the HTML splash out instead.
	const skipOverlay = initialPhaseRef.current === "ready"
	useEffect(() => {
		const splash = document.getElementById("splash")
		if (!splash) return

		if (skipOverlay) {
			// No React overlay will render, fade out the HTML splash
			splash.classList.add("hiding")
			setTimeout(() => splash.remove(), 300)
		} else {
			// React overlay is covering the screen, remove HTML splash instantly
			splash.remove()
		}
	}, [skipOverlay])

	// When discovery reaches "ready", start the fade-out then unmount.
	useEffect(() => {
		if (phase !== "ready") return
		setState("fading")
	}, [phase])

	// Once fading, wait for the CSS transition to finish then unmount.
	useEffect(() => {
		if (state !== "fading") return
		const timer = setTimeout(() => setState("unmounted"), FADE_DURATION_MS)
		return () => clearTimeout(timer)
	}, [state])

	if (state === "unmounted") return null

	const isVisible = state === "visible"

	return (
		<div
			data-slot="startup-overlay"
			className={`fixed inset-0 z-[9999] flex flex-col items-center justify-center transition-opacity ${isVisible ? "opacity-100" : "opacity-0"}`}
			style={{
				transitionDuration: `${FADE_DURATION_MS}ms`,
				transitionTimingFunction: "ease-out",
				pointerEvents: isVisible ? "auto" : "none",
				// Allow dragging the window from the overlay on macOS
				// @ts-expect-error -- vendor-prefixed CSS property
				WebkitAppRegion: "drag",
			}}
		>
			<DevoSplashBrand />
			<div className="devo-loading-dots mt-8" aria-hidden="true">
				<span />
				<span />
				<span />
			</div>
		</div>
	)
}
