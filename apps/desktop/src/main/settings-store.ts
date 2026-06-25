import fs from "node:fs"
import path from "node:path"
import { app } from "electron"
import type { AppSettings, NotificationSettings } from "../preload/api"
import { DEFAULT_APP_SETTINGS } from "../shared/app-settings"
import { createLogger } from "./logger"

const log = createLogger("settings-store")

export type { AppSettings, NotificationSettings }

// ============================================================
// Defaults
// ============================================================

const DEFAULT_SETTINGS: AppSettings = DEFAULT_APP_SETTINGS

// ============================================================
// State
// ============================================================

let settings: AppSettings = structuredClone(DEFAULT_SETTINGS)
let settingsPath: string | null = null

/** Listeners notified when settings change (main-process internal). */
const listeners = new Set<(settings: AppSettings) => void>()

// ============================================================
// Public API
// ============================================================

/** Initialize the settings store. Call once from app.whenReady(). */
export function initSettingsStore(): void {
	const configDir = app.getPath("userData")
	settingsPath = path.join(configDir, "settings.json")

	try {
		if (fs.existsSync(settingsPath)) {
			const raw = fs.readFileSync(settingsPath, "utf-8")
			const parsed = JSON.parse(raw)
			// Deep merge with defaults to handle new fields added in updates
			settings = deepMerge(structuredClone(DEFAULT_SETTINGS), parsed)
			log.info("Settings loaded", { path: settingsPath })
		} else {
			log.info("No settings file found, using defaults", {
				path: settingsPath,
			})
		}
	} catch (err) {
		log.error("Failed to load settings, using defaults", err)
	}

	// Migrate opaqueWindows from the old preferences.json into settings.json
	migrateFromPreferencesJson(configDir)
}

/**
 * One-time migration: move `opaqueWindows` from the legacy `preferences.json`
 * into the unified `settings.json`. Deletes the old file after migration.
 */
function migrateFromPreferencesJson(configDir: string): void {
	const oldPath = path.join(configDir, "preferences.json")
	try {
		if (!fs.existsSync(oldPath)) return
		const raw = fs.readFileSync(oldPath, "utf-8")
		const parsed = JSON.parse(raw)
		if (
			typeof parsed.opaqueWindows === "boolean" &&
			parsed.opaqueWindows !== settings.opaqueWindows
		) {
			settings = { ...settings, opaqueWindows: parsed.opaqueWindows }
			persist()
			log.info("Migrated opaqueWindows from preferences.json")
		}
		// Remove old file regardless
		fs.unlinkSync(oldPath)
		log.info("Removed legacy preferences.json")
	} catch (err) {
		log.warn("Failed to migrate from preferences.json (non-fatal)", err)
	}
}

/** Get the full settings object (synchronous — reads from memory). */
export function getSettings(): AppSettings {
	return settings
}

/** Get just the notification settings (convenience for the notification system). */
export function getNotificationSettings(): NotificationSettings {
	return settings.notifications
}

/** Get the opaque windows preference (read at window creation time). */
export function getOpaqueWindows(): boolean {
	return settings.opaqueWindows
}

/** Update settings with a partial object. Deep-merges and persists to disk. */
export function updateSettings(partial: DeepPartial<AppSettings>): AppSettings {
	settings = deepMerge(settings, partial)
	persist()
	notifyListeners()
	return settings
}

/** Subscribe to settings changes. Returns an unsubscribe function. */
export function onSettingsChanged(listener: (settings: AppSettings) => void): () => void {
	listeners.add(listener)
	return () => listeners.delete(listener)
}

// ============================================================
// Internal
// ============================================================

function persist(): void {
	if (!settingsPath) return
	try {
		const dir = path.dirname(settingsPath)
		if (!fs.existsSync(dir)) {
			fs.mkdirSync(dir, { recursive: true })
		}
		const tmpPath = `${settingsPath}.tmp`
		fs.writeFileSync(tmpPath, JSON.stringify(settings, null, "\t"), "utf-8")
		fs.renameSync(tmpPath, settingsPath)
		log.debug("Settings persisted", { path: settingsPath })
	} catch (err) {
		log.error("Failed to persist settings", err)
	}
}

function notifyListeners(): void {
	for (const listener of listeners) {
		try {
			listener(settings)
		} catch (err) {
			log.error("Settings listener error", err)
		}
	}
}

// ============================================================
// Utility types and functions
// ============================================================

export type DeepPartial<T> = {
	[P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P]
}

const UNSAFE_KEYS = new Set(["__proto__", "constructor", "prototype"])

// biome-ignore lint/suspicious/noExplicitAny: internal merge utility operates on arbitrary shapes
function deepMerge(target: any, source: any): any {
	const result = { ...target }
	for (const key of Object.keys(source)) {
		if (UNSAFE_KEYS.has(key)) continue
		const sourceVal = source[key]
		const targetVal = target[key]
		if (
			sourceVal !== null &&
			sourceVal !== undefined &&
			typeof sourceVal === "object" &&
			!Array.isArray(sourceVal) &&
			typeof targetVal === "object" &&
			targetVal !== null &&
			!Array.isArray(targetVal)
		) {
			result[key] = deepMerge(targetVal, sourceVal)
		} else if (sourceVal !== undefined) {
			result[key] = sourceVal
		}
	}
	return result
}
