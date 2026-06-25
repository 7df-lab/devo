import { atom } from "jotai"
import type { ServerConfig } from "../../preload/api"
import { DEFAULT_LOCAL_SERVER, DEFAULT_SERVER_SETTINGS } from "../../shared/server-config"

// ============================================================
// Server configuration atoms (persisted via settings)
// ============================================================

/** All configured servers. Initialized from settings on app start. */
export const serversAtom = atom<ServerConfig[]>(DEFAULT_SERVER_SETTINGS.servers)

/** ID of the currently active server. */
export const activeServerIdAtom = atom<string>(DEFAULT_SERVER_SETTINGS.activeServerId)

/** Derived: the active server config object. Falls back to local if ID not found. */
export const activeServerConfigAtom = atom<ServerConfig>((get) => {
	const servers = get(serversAtom)
	const activeId = get(activeServerIdAtom)
	return servers.find((s) => s.id === activeId) ?? DEFAULT_LOCAL_SERVER
})

// ============================================================
// Live connection state atoms
// ============================================================

/** The URL of the currently connected server (set after connection is established). */
export const serverUrlAtom = atom<string | null>(null)

/** Whether we are currently connected. */
export const serverConnectedAtom = atom<boolean>(false)

/** Auth header for the current connection (null for local/unauthenticated). */
export const authHeaderAtom = atom<string | null>(null)

/** Derived convenience atom for components that need connection + server info. */
export const connectionAtom = atom((get) => ({
	url: get(serverUrlAtom),
	connected: get(serverConnectedAtom),
	activeServer: get(activeServerConfigAtom),
}))
