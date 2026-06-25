import { useCallback, useEffect, useState } from "react"
import type { AppSettings } from "../../preload/api"
import { DEFAULT_APP_SETTINGS } from "../../shared/app-settings"

const isElectron = typeof window !== "undefined" && "devo" in window

const DEFAULT_SETTINGS: AppSettings = DEFAULT_APP_SETTINGS

export function useSettings() {
	const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS)
	const [loading, setLoading] = useState(true)

	useEffect(() => {
		if (!isElectron) {
			setLoading(false)
			return
		}
		window.devo
			.getSettings()
			.then((s) => {
				setSettings(s as AppSettings)
			})
			.catch((err) => {
				console.error("Failed to load settings:", err)
			})
			.finally(() => {
				setLoading(false)
			})
	}, [])

	// Listen for settings changes pushed from the main process.
	// This ensures the renderer stays in sync if settings change externally
	// (e.g. notification action buttons update a setting from the main process).
	useEffect(() => {
		if (!isElectron) return
		return window.devo.onSettingsChanged((updated) => {
			setSettings(updated)
		})
	}, [])

	const updateSettings = useCallback(
		async (partial: Record<string, unknown>) => {
			if (!isElectron) return
			const prev = settings
			try {
				const updated = (await window.devo.updateSettings(partial)) as AppSettings
				setSettings(updated)
			} catch (err) {
				console.error("Failed to update settings:", err)
				setSettings(prev)
			}
		},
		[settings],
	)

	return { settings, loading, updateSettings }
}
