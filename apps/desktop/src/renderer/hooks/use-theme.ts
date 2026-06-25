import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useLayoutEffect, useMemo } from "react"
import { colorSchemeAtom, themeAtom } from "../atoms/preferences"
import { persistAppearanceSettings } from "../lib/settings-sync"
import { type ColorScheme, getAvailableThemes, getTheme, type ThemeDefinition } from "../lib/themes"

// ============================================================
// useThemeEffect — synchronises persisted store to <html> element
// ============================================================

const STYLE_ID = "devo-theme-vars"

function getOrCreateStyleElement(): HTMLStyleElement {
	let el = document.getElementById(STYLE_ID) as HTMLStyleElement | null
	if (!el) {
		el = document.createElement("style")
		el.id = STYLE_ID
		document.head.appendChild(el)
	}
	return el
}

function resolveColorSchemeClass(scheme: ColorScheme): "dark" | "light" {
	if (scheme === "system") {
		return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
	}
	return scheme
}

function buildGlassVars(theme: ThemeDefinition): [string, string][] {
	const g = theme.glass
	if (!g) return []
	const vars: [string, string][] = []
	if (g.bodyOpacity !== undefined) vars.push(["--glass-body", `${g.bodyOpacity}%`])
	if (g.sidebarOpacity !== undefined) vars.push(["--glass-sidebar", `${g.sidebarOpacity}%`])
	if (g.surfaceOpacity !== undefined) vars.push(["--glass-surface", `${g.surfaceOpacity}%`])
	if (g.elevatedOpacity !== undefined) vars.push(["--glass-elevated", `${g.elevatedOpacity}%`])
	if (g.cardOpacity !== undefined) vars.push(["--glass-card", `${g.cardOpacity}%`])
	if (g.contentOpacity !== undefined) vars.push(["--glass-content", `${g.contentOpacity}%`])
	if (g.blurScale !== undefined) {
		const s = g.blurScale
		vars.push(["--blur-sm", `${8 * s}px`])
		vars.push(["--blur-md", `${12 * s}px`])
		vars.push(["--blur-lg", `${16 * s}px`])
		vars.push(["--blur-xl", `${24 * s}px`])
	}
	return vars
}

function buildThemeCss(theme: ThemeDefinition): string {
	const lightEntries = Object.entries(theme.cssVars.light)
	const darkEntries = Object.entries(theme.cssVars.dark)
	const densityEntries = theme.density ? Object.entries(theme.density) : []
	const radiusEntry = theme.radius ? [["--radius", theme.radius] as const] : []
	const glassEntries = buildGlassVars(theme)

	// Glass entries are defaults — cssVars entries override them (come last)
	const allLight = [...glassEntries, ...densityEntries, ...radiusEntry, ...lightEntries]
	const allDark = [...glassEntries, ...densityEntries, ...radiusEntry, ...darkEntries]

	if (allLight.length === 0 && allDark.length === 0) return ""

	let css = ""
	if (allLight.length > 0) {
		css += `:root {\n${allLight.map(([k, v]) => `  ${k}: ${v};`).join("\n")}\n}\n`
	}
	if (allDark.length > 0) {
		css += `.dark {\n${allDark.map(([k, v]) => `  ${k}: ${v};`).join("\n")}\n}\n`
	}
	return css
}

export function useThemeEffect() {
	const themeId = useAtomValue(themeAtom)
	const colorScheme = useAtomValue(colorSchemeAtom)

	const theme = useMemo(() => getTheme(themeId), [themeId])

	useLayoutEffect(() => {
		const root = document.documentElement

		const cls = resolveColorSchemeClass(colorScheme)
		root.classList.remove("dark", "light")
		root.classList.add(cls)

		for (const c of Array.from(root.classList)) {
			if (c.startsWith("theme-")) root.classList.remove(c)
		}
		if (theme.id !== "default") {
			root.classList.add(`theme-${theme.id}`)
		}

		// If the theme disables glass, force opaque regardless of platform tier
		if (theme.glass?.disabled) {
			root.classList.remove("electron-transparent", "electron-vibrancy")
			root.classList.add("electron-opaque")
		}

		const styleEl = getOrCreateStyleElement()
		styleEl.textContent = buildThemeCss(theme)

		// Sync native theme with OS chrome so macOS glass tint and Windows window
		// control symbols match the app's CSS color scheme.
		if ("devo" in window) {
			window.devo.setNativeTheme(colorScheme === "system" ? "system" : cls)
		}

		if (theme.fonts?.sans) {
			root.style.setProperty("--font-sans", theme.fonts.sans)
		} else {
			root.style.removeProperty("--font-sans")
		}
		if (theme.fonts?.mono) {
			root.style.setProperty("--font-mono", theme.fonts.mono)
		} else {
			root.style.removeProperty("--font-mono")
		}

		if (colorScheme === "system") {
			const mq = window.matchMedia("(prefers-color-scheme: dark)")
			const handler = (e: MediaQueryListEvent) => {
				root.classList.remove("dark", "light")
				root.classList.add(e.matches ? "dark" : "light")
			}
			mq.addEventListener("change", handler)
			return () => mq.removeEventListener("change", handler)
		}
	}, [theme, colorScheme])
}

// ============================================================
// Convenience hooks
// ============================================================

export function useCurrentTheme(): ThemeDefinition {
	const themeId = useAtomValue(themeAtom)
	return useMemo(() => getTheme(themeId), [themeId])
}

export function useColorScheme(): ColorScheme {
	return useAtomValue(colorSchemeAtom)
}

export function useAvailableThemes(): ThemeDefinition[] {
	const platform =
		typeof window !== "undefined" && "devo" in window ? window.devo.platform : undefined
	return useMemo(() => getAvailableThemes(platform), [platform])
}

export function useSetTheme(): (id: string) => void {
	const setTheme = useSetAtom(themeAtom)
	return useCallback(
		(id: string) => {
			setTheme(id)
			void persistAppearanceSettings({ themeId: id })
		},
		[setTheme],
	)
}

export function useSetColorScheme(): (scheme: ColorScheme) => void {
	const setColorScheme = useSetAtom(colorSchemeAtom)
	return useCallback(
		(scheme: ColorScheme) => {
			setColorScheme(scheme)
			void persistAppearanceSettings({ colorScheme: scheme })
		},
		[setColorScheme],
	)
}
