// ============================================================
// Theme definitions for Devo
//
// Each theme defines CSS custom property overrides for light and
// dark modes plus optional font, radius, and density changes.
//
// Architecture follows shadcn/ui's convention:
//   - `:root` / `.dark` define the default theme (= Cortex)
//   - Named themes (`.theme-<id>`) override the same variables
//   - `@theme inline` in globals.css bridges vars to Tailwind
//   - useThemeEffect applies classes + CSS vars to <html>
// ============================================================

/**
 * Color scheme preference.
 *  - "dark" / "light" — explicit
 *  - "system" — follows prefers-color-scheme
 */
export type ColorScheme = "dark" | "light" | "system"

/**
 * A theme definition. Every field except `id`, `name`, and
 * `cssVars` is optional — unset values inherit from defaults.
 */
export interface ThemeDefinition {
	/** Unique identifier, used as CSS class: `theme-<id>` */
	id: string
	/** Human-readable label shown in the command palette */
	name: string
	/** Optional description */
	description?: string

	/**
	 * Restrict this theme to specific platforms.
	 * If omitted, the theme is available everywhere.
	 * Uses Node.js platform strings: "darwin", "win32", "linux".
	 */
	platforms?: NodeJS.Platform[]

	/**
	 * CSS custom property overrides.  Only properties that differ
	 * from the default theme need to be listed.
	 */
	cssVars: {
		light: Record<string, string>
		dark: Record<string, string>
	}

	/**
	 * Font stack overrides.  Applied via
	 * `document.documentElement.style.setProperty("--font-sans", ...)`.
	 * If omitted the default theme fonts remain.
	 */
	fonts?: {
		sans?: string
		mono?: string
	}

	/**
	 * Base border-radius override.  Applied as `--radius`.
	 * Other radius tokens (sm/md/lg/xl) are derived from this.
	 */
	radius?: string

	/**
	 * Text size / density overrides. Applied as --text-xs, --text-sm etc.
	 * Themes like Cortex use a tighter 13px base.
	 */
	density?: {
		"--text-xs"?: string
		"--text-xs--line-height"?: string
		"--text-sm"?: string
		"--text-sm--line-height"?: string
	}

	/**
	 * Glass transparency tuning. Only takes effect when the window has
	 * native or BrowserWindow transparency. Themes can adjust
	 * opacity per surface, blur intensity, or disable glass entirely.
	 *
	 * Values override the CSS custom properties defined in globals.css.
	 * Unset fields inherit the defaults.
	 */
	glass?: {
		/** Body background opacity (0–100). Default: 50 */
		bodyOpacity?: number
		/** Sidebar panel opacity (0–100). Default: 70 */
		sidebarOpacity?: number
		/** App bar / divider surface opacity (0–100). Default: 80 */
		surfaceOpacity?: number
		/** Floating panel opacity: popovers, dialogs, command palette (0–100). Default: 85 */
		elevatedOpacity?: number
		/** Card / inline panel opacity (0–100). Default: 92 */
		cardOpacity?: number
		/** Main content area opacity (0–100). Default: 80 */
		contentOpacity?: number
		/** Blur multiplier (1.0 = default, 0.5 = half, 2.0 = double). Default: 1.0 */
		blurScale?: number
		/** Disable glass for this theme entirely, forcing opaque even on macOS. Default: false */
		disabled?: boolean
	}
}

// ============================================================
// Cortex theme (default) — cool neutral palette
//
// This is the default theme. The values in globals.css match
// exactly, so no CSS var overrides are needed — they are the
// baseline that other themes override.
//
// Key traits:
//   - Neutral (cool) grays
//   - Devo Blue accent (#05bdf5 base, oklch 228.71 hue)
//   - System fonts (SF Pro on macOS, Segoe UI on Windows)
//   - Squircle-style 12px border-radius
//   - Tight 13px text density
// ============================================================

export const cortexTheme: ThemeDefinition = {
	id: "cortex",
	name: "Cortex",
	description: "Cool neutrals with Devo Blue accent, clean and minimal",
	cssVars: {
		light: {
			"--glass-sidebar-accent": "35%",
		},
		dark: {
			"--glass-elevated": "95%",
			"--glass-sidebar-accent": "35%",
		},
	},
	glass: {
		bodyOpacity: 38,
		sidebarOpacity: 18,
		surfaceOpacity: 55,
		elevatedOpacity: 55,
		cardOpacity: 88,
	},
}

// ============================================================
// Liquid Glass theme — macOS only, maximally transparent
//
// Key traits:
//   - Extremely low surface opacities — the desktop bleeds
//     through almost everything
//   - Higher blur multiplier to keep text readable despite
//     the near-invisible backgrounds
//   - System fonts (SF Pro on macOS)
//   - macOS-only: requires native transparency (liquid glass
//     or vibrancy) to look correct
// ============================================================

export const liquidGlassTheme: ThemeDefinition = {
	id: "liquid-glass",
	name: "Liquid Glass",
	description: "Extremely transparent — lets your wallpaper shine through",
	platforms: ["darwin"],
	cssVars: {
		light: {
			"--glass-body": "12%",
			"--glass-sidebar": "10%",
			"--glass-surface": "18%",
			"--glass-elevated": "30%",
			"--glass-card": "15%",
			"--border": "rgba(0, 0, 0, 0.08)",
			"--sidebar-border": "rgba(0, 0, 0, 0.06)",
		},
		dark: {
			"--glass-body": "10%",
			"--glass-sidebar": "8%",
			"--glass-surface": "15%",
			"--glass-elevated": "28%",
			"--glass-card": "15%",
			"--border": "rgba(255, 255, 255, 0.10)",
			"--sidebar-border": "rgba(255, 255, 255, 0.08)",
			/* Boost text contrast — wallpaper bleeds through and washes out
			   the default muted grays. Push everything brighter. */
			"--foreground": "#ffffff",
			"--muted-foreground": "#b0aead",
			"--card-foreground": "#ffffff",
			"--popover-foreground": "#ffffff",
			"--accent-foreground": "#ffffff",
			"--secondary-foreground": "#ffffff",
			"--sidebar-foreground": "#ffffff",
			"--sidebar-accent-foreground": "#ffffff",
		},
	},
	fonts: {
		sans: '-apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", system-ui, sans-serif',
		mono: '"SF Mono", ui-monospace, "SFMono-Regular", Menlo, monospace',
	},
	radius: "0.75rem",
	glass: {
		bodyOpacity: 10,
		sidebarOpacity: 8,
		surfaceOpacity: 15,
		elevatedOpacity: 28,
		cardOpacity: 15,
		contentOpacity: 15,
		blurScale: 2.0,
	},
}

// ============================================================
// System theme — uses the OS accent color
//
// Key traits:
//   - Inherits the Cortex base palette
//   - Accent-related variables reference --system-accent with
//     Devo Blue fallbacks, so they track the OS accent color
//     automatically (set on :root by useSystemAccentColor)
// ============================================================

export const systemTheme: ThemeDefinition = {
	id: "default",
	name: "System",
	description: "Adapts accent colors to your OS settings",
	cssVars: {
		light: {
			"--ring": "var(--system-accent, #0080bd)",
			"--chart-1": "var(--system-accent, #05bdf5)",
			"--sidebar-accent": "var(--system-accent-light, #a8ddf7)",
			"--sidebar-ring": "var(--system-accent, #0080bd)",
			"--glass-sidebar-accent": "35%",
		},
		dark: {
			"--ring": "var(--system-accent, #6fcbf3)",
			"--chart-1": "var(--system-accent, #05bdf5)",
			"--sidebar-accent": "var(--system-accent-dark, #001a2b)",
			"--sidebar-ring": "var(--system-accent, #6fcbf3)",
			"--glass-elevated": "95%",
			"--glass-sidebar-accent": "35%",
		},
	},
	glass: {
		bodyOpacity: 0,
		sidebarOpacity: 18,
		surfaceOpacity: 55,
		elevatedOpacity: 55,
		cardOpacity: 88,
	},
}

// ============================================================
// Theme registry — add new themes here
// ============================================================

export const themes: ThemeDefinition[] = [systemTheme, cortexTheme, liquidGlassTheme]

/**
 * Return only themes available on the given platform.
 * Themes without a `platforms` restriction are always included.
 */
export function getAvailableThemes(platform?: NodeJS.Platform): ThemeDefinition[] {
	if (!platform) return themes
	return themes.filter((t) => !t.platforms || t.platforms.includes(platform))
}

/**
 * Look up a theme by id.  Falls back to default if not found.
 */
export function getTheme(id: string): ThemeDefinition {
	return themes.find((t) => t.id === id) ?? systemTheme
}
