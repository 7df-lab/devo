import { useCallback, useEffect, useState } from "react"
import type { UpdateState } from "../../preload/api"
import { isElectron } from "../services/backend"

const defaultState: UpdateState = { status: "idle", canAutoInstall: true }

/**
 * Hook that tracks the auto-updater state from the main process.
 * Subscribes to push events via the preload bridge and provides
 * action helpers for check / download / install.
 *
 * On unsigned macOS builds, `canAutoInstall` will be false and the
 * "install" action is replaced with opening the GitHub release page.
 *
 * In browser mode (non-Electron), always returns idle state and no-op actions.
 */
export function useUpdater() {
	const [state, setState] = useState<UpdateState>(defaultState)

	// Fetch initial state and subscribe to changes
	useEffect(() => {
		if (!isElectron) return

		// Get current state on mount
		window.devo
			.getUpdateState()
			.then(setState)
			.catch(() => {})

		// Subscribe to state changes pushed from main process
		const unsubscribe = window.devo.onUpdateStateChanged((newState) => {
			setState(newState)
		})

		return unsubscribe
	}, [])

	const checkForUpdates = useCallback(async () => {
		if (!isElectron) return
		await window.devo.checkForUpdates()
	}, [])

	const downloadUpdate = useCallback(async () => {
		if (!isElectron) return
		await window.devo.downloadUpdate()
	}, [])

	const installUpdate = useCallback(() => {
		if (!isElectron) return
		window.devo.installUpdate()
	}, [])

	const openReleasePage = useCallback(async () => {
		if (!isElectron) return
		await window.devo.openReleasePage()
	}, [])

	return {
		...state,
		checkForUpdates,
		downloadUpdate,
		installUpdate,
		openReleasePage,
	}
}
