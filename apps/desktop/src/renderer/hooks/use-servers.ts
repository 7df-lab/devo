/**
 * Local-only server configuration hook.
 *
 * Desktop v1 owns a single main-process Native stdio connection. Remote URL,
 * service discovery, password, and SSH server switching are intentionally disabled.
 */

import { useSetAtom } from "jotai"
import { useEffect } from "react"
import { DEFAULT_LOCAL_SERVER, DEFAULT_SERVER_SETTINGS } from "../../shared/server-config"
import { activeServerIdAtom, serversAtom } from "../atoms/connection"
import { isElectron } from "../services/backend"

/**
 * Syncs local server settings from the main process into Jotai atoms.
 * Existing remote settings are normalized out so this build always uses stdio.
 */
export function useServerSettingsSync() {
	const setServers = useSetAtom(serversAtom)
	const setActiveServerId = useSetAtom(activeServerIdAtom)

	useEffect(() => {
		setServers(DEFAULT_SERVER_SETTINGS.servers)
		setActiveServerId(DEFAULT_SERVER_SETTINGS.activeServerId)

		if (!isElectron) return

		window.devo.getSettings().then((settings) => {
			if (
				settings.servers?.activeServerId !== DEFAULT_SERVER_SETTINGS.activeServerId ||
				settings.servers?.servers.length !== 1 ||
				settings.servers.servers[0]?.id !== DEFAULT_LOCAL_SERVER.id
			) {
				void window.devo.updateSettings({ servers: DEFAULT_SERVER_SETTINGS })
			}
		})

		return window.devo.onSettingsChanged(() => {
			setServers(DEFAULT_SERVER_SETTINGS.servers)
			setActiveServerId(DEFAULT_SERVER_SETTINGS.activeServerId)
		})
	}, [setServers, setActiveServerId])
}
