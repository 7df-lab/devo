import type { AppSettings, AppearanceSettings } from "../../preload/api"

type AppearancePatch = Partial<Omit<AppearanceSettings, "rendererPreferencesMigrated">>

const COLOR_SCHEMES = new Set(["dark", "light", "system"])
const DISPLAY_MODES = new Set(["default", "verbose"])

function readJsonStorageValue(storage: Storage, key: string): unknown {
	const raw = storage.getItem(key)
	if (!raw) return undefined
	try {
		return JSON.parse(raw)
	} catch {
		return undefined
	}
}

function readRendererAppearanceSnapshot(storage: Storage): AppearancePatch {
	const colorScheme = readJsonStorageValue(storage, "devo:colorScheme")
	const themeId = readJsonStorageValue(storage, "devo:theme")
	const displayMode = readJsonStorageValue(storage, "devo:displayMode")

	const snapshot: AppearancePatch = {}
	if (typeof colorScheme === "string" && COLOR_SCHEMES.has(colorScheme)) {
		snapshot.colorScheme = colorScheme as AppearanceSettings["colorScheme"]
	}
	if (typeof themeId === "string" && themeId.length > 0) {
		snapshot.themeId = themeId
	}
	if (typeof displayMode === "string" && DISPLAY_MODES.has(displayMode)) {
		snapshot.displayMode = displayMode as AppearanceSettings["displayMode"]
	}
	return snapshot
}

export function buildRendererPreferencesMigrationPatch(
	settings: AppSettings,
	storage: Storage,
): { appearance: Partial<AppearanceSettings> } | null {
	if (settings.appearance.rendererPreferencesMigrated) return null

	return buildAppearanceSettingsPatch({
		...readRendererAppearanceSnapshot(storage),
		rendererPreferencesMigrated: true,
	})
}

export function buildAppearanceSettingsPatch(
	appearance: Partial<AppearanceSettings>,
): { appearance: Partial<AppearanceSettings> } {
	return {
		appearance: {
			...appearance,
			rendererPreferencesMigrated: true,
		},
	}
}

export async function persistAppearanceSettings(
	appearance: Partial<AppearanceSettings>,
): Promise<void> {
	if (typeof window === "undefined" || !("devo" in window)) return
	await window.devo.updateSettings(buildAppearanceSettingsPatch(appearance))
}
