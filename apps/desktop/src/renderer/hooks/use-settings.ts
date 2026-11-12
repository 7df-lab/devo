import { useCallback, useEffect, useRef, useState } from "react"
import type { AppSettings } from "../../preload/api"
import { DEFAULT_APP_SETTINGS } from "../../shared/app-settings"

const isElectron = typeof window !== "undefined" && "devo" in window

export function useSettings() {
	const [settings, setSettings] = useState<AppSettings>(DEFAULT_APP_SETTINGS)
	const [loading, setLoading] = useState(true)
	const lifecycle = useRef({ active: false, revision: 0, mutation: 0 })

	useEffect(() => {
		const current = { active: true, revision: 0, mutation: 0 }
		lifecycle.current = current
		if (!isElectron) {
			setLoading(false)
			return () => {
				current.active = false
			}
		}
		const unsubscribe = window.devo.onSettingsChanged((updated) => {
			if (!current.active) return
			current.revision++
			setSettings(updated)
			setLoading(false)
		})
		const revision = current.revision
		window.devo
			.getSettings()
			.then((s) => {
				if (current.active && current.revision === revision) setSettings(s)
			})
			.catch((err) => {
				if (current.active) console.error("Failed to load settings:", err)
			})
			.finally(() => {
				if (current.active) setLoading(false)
			})
		return () => {
			current.active = false
			unsubscribe()
		}
	}, [])

	const updateSettings = useCallback(async (partial: Record<string, unknown>) => {
		const current = lifecycle.current
		if (!isElectron || !current.active) return
		const revision = current.revision
		const mutation = ++current.mutation
		try {
			const updated = await window.devo.updateSettings(partial)
			// A live push or a newer mutation supersedes this response.
			if (current.active && current.revision === revision && current.mutation === mutation) {
				current.revision++
				setSettings(updated)
			}
		} catch (err) {
			// Nothing was optimistically applied, so rolling back would erase newer state.
			if (current.active) console.error("Failed to update settings:", err)
		}
	}, [])

	return { settings, loading, updateSettings }
}
