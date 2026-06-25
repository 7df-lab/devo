/**
 * Sets --system-accent, --system-accent-light, and --system-accent-dark
 * CSS custom properties on :root from the OS accent color.
 *
 * Runs unconditionally on mount and subscribes to live changes.
 * Themes reference these variables with fallbacks, so they degrade
 * gracefully when the accent color is unavailable (browser mode, Linux).
 */

import { useEffect } from "react"

/**
 * Parse an RRGGBBAA hex string (from Electron's systemPreferences.getAccentColor())
 * into { r, g, b } integers 0-255.
 */
function parseAccentHex(hex: string): { r: number; g: number; b: number } | null {
	if (hex.length < 6) return null
	const r = Number.parseInt(hex.slice(0, 2), 16)
	const g = Number.parseInt(hex.slice(2, 4), 16)
	const b = Number.parseInt(hex.slice(4, 6), 16)
	if (Number.isNaN(r) || Number.isNaN(g) || Number.isNaN(b)) return null
	return { r, g, b }
}

/**
 * Mix a color towards white by a given factor (0-1).
 */
function lighten(r: number, g: number, b: number, factor: number): string {
	const lr = Math.round(r + (255 - r) * factor)
	const lg = Math.round(g + (255 - g) * factor)
	const lb = Math.round(b + (255 - b) * factor)
	return `#${lr.toString(16).padStart(2, "0")}${lg.toString(16).padStart(2, "0")}${lb.toString(16).padStart(2, "0")}`
}

/**
 * Mix a color towards black by a given factor (0-1).
 */
function darken(r: number, g: number, b: number, factor: number): string {
	const dr = Math.round(r * (1 - factor))
	const dg = Math.round(g * (1 - factor))
	const db = Math.round(b * (1 - factor))
	return `#${dr.toString(16).padStart(2, "0")}${dg.toString(16).padStart(2, "0")}${db.toString(16).padStart(2, "0")}`
}

function applyAccentColor(hex: string): void {
	const rgb = parseAccentHex(hex)
	if (!rgb) return

	const root = document.documentElement
	root.style.setProperty("--system-accent", `#${hex.slice(0, 6)}`)
	root.style.setProperty("--system-accent-light", lighten(rgb.r, rgb.g, rgb.b, 0.7))
	root.style.setProperty("--system-accent-dark", darken(rgb.r, rgb.g, rgb.b, 0.8))
}

export function useSystemAccentColor(): void {
	useEffect(() => {
		if (!("devo" in window)) return

		// Read initial value
		window.devo.getAccentColor().then((color) => {
			if (color) applyAccentColor(color)
		})

		// Subscribe to live changes
		const unsubscribe = window.devo.onAccentColorChanged((color) => {
			applyAccentColor(color)
		})

		return unsubscribe
	}, [])
}
