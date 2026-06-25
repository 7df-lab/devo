import { useAtomValue } from "jotai"
import { useEffect } from "react"
import { activeServerConfigAtom, serverConnectedAtom } from "../atoms/connection"
import { discoveryAtom } from "../atoms/discovery"
import { isMockModeAtom } from "../atoms/mock-mode"
import { appStore } from "../atoms/store"
import { createLogger } from "../lib/logger"
import { resolveAuthHeader, resolveServerUrl } from "../services/backend"
import {
	connectToDevo,
	loadAllProjects,
	loadProjectSessions,
} from "../services/connection-manager"

const log = createLogger("discovery")

// Module-level guard to prevent concurrent discovery runs.
// The Jotai atom guard (loaded/loading) depends on a React re-render
// to propagate, which can race with React Strict Mode double-effects
// or fast re-mounts.
let discoveryInFlight = false

/** Reset the discovery guard so discovery can re-run (used when switching servers or exiting mock mode). */
export function resetDiscoveryGuard(): void {
	discoveryInFlight = false
}

/** Helper to update the discovery phase without touching other fields. */
function setPhase(phase: import("../atoms/discovery").DiscoveryPhase): void {
	appStore.set(discoveryAtom, (prev) => ({ ...prev, phase }))
}

/**
 * API-first discovery hook.
 *
 * On mount:
 * 1. Resolves the active server URL (starts the local stdio runtime)
 * 2. Resolves auth credentials if the server requires them
 * 3. Connects to the Devo server (ACP events for all projects)
 * 4. Lists all projects from the API via `client.project.list()`
 * 5. Loads sessions for the top few most-recently-active projects
 *    (enough to populate "Recent" and "Active Now" sections)
 *
 * Remaining project sessions are loaded lazily when expanded in the sidebar.
 * Active sessions also arrive in real-time via ACP events.
 */
export function useDiscovery() {
	const discovery = useAtomValue(discoveryAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const activeServer = useAtomValue(activeServerConfigAtom)
	const { loaded, loading } = discovery

	useEffect(() => {
		// In mock mode, atoms are hydrated by useMockMode() -- skip real discovery
		if (isMockMode) return
		if (loaded || loading || discoveryInFlight) return
		discoveryInFlight = true

		// Set loading
		appStore.set(discoveryAtom, (prev) => ({
			...prev,
			loading: true,
			error: null,
			phase: "starting-server",
		}))

		;(async () => {
			try {
				// --- Step 1: Resolve the server URL ---
				log.info("Resolving server URL...", {
					server: activeServer.name,
					type: activeServer.type,
				})
				const url = await resolveServerUrl(activeServer)

				// --- Step 2: Resolve auth if needed ---
				const authHeader = await resolveAuthHeader(activeServer)

				// --- Step 3: Connect to the server (starts ACP event loop) ---
				setPhase("connecting")
				log.info("Connecting to Devo server", {
					url,
					server: activeServer.name,
					authenticated: !!authHeader,
				})
				await connectToDevo(url, authHeader)

				// --- Step 3b: Bail if server is unreachable ---
				// connectToDevo runs a health check and sets serverConnectedAtom.
				// If the server is offline, skip project/session loading so discovery
				// stays in a non-loaded state, allowing the sidebar to show "Server offline".
				// Keep discoveryInFlight = true to prevent an infinite retry loop;
				// resetDiscoveryGuard() (called on server switch) clears it.
				if (!appStore.get(serverConnectedAtom)) {
					log.warn("Server is unreachable, skipping project discovery", {
						server: activeServer.name,
					})
					appStore.set(discoveryAtom, (prev) => ({
						...prev,
						loading: false,
						error: "Server offline",
						phase: "error",
					}))
					return
				}

				// --- Step 4: Discover projects from the API ---
				setPhase("loading-projects")
				log.info("Loading projects from API...")
				const projects = await loadAllProjects()
				log.info("Discovered projects via API", { count: projects.length })

				// Store projects and mark discovery as complete.
				// Remaining sessions are loaded lazily when the user expands a project.
				appStore.set(discoveryAtom, {
					loaded: true,
					loading: false,
					error: null,
					phase: "ready",
					projects,
				})

				// --- Step 5: Pre-fetch sessions for the most recent projects ---
				// Load sessions from the top N most-recently-active projects so the
				// "Recent" and "Active Now" sidebar sections are populated at boot.
				// This replaces the previous approach of loading ALL projects (80+ calls)
				// with just a few calls for the projects the user likely cares about.
				const PREFETCH_COUNT = 3
				const sortedByActivity = [...projects]
					.filter((p) => p.worktree)
					.sort((a, b) => (b.time.updated ?? 0) - (a.time.updated ?? 0))
					.slice(0, PREFETCH_COUNT)

				if (sortedByActivity.length > 0) {
					// Build sandbox lookup for worktree metadata restoration
					const projectSandboxMap = new Map<string, Set<string>>()
					for (const project of projects) {
						if (!project.worktree || !project.sandboxes?.length) continue
						const sandboxSet = new Set<string>()
						for (const s of project.sandboxes) sandboxSet.add(s)
						projectSandboxMap.set(project.worktree, sandboxSet)
					}

					await Promise.allSettled(
						sortedByActivity.map((project) => {
							const sandboxDirs = projectSandboxMap.get(project.worktree!)
							return loadProjectSessions(
								project.worktree!,
								sandboxDirs?.size ? sandboxDirs : undefined,
								{ limit: 5, roots: true },
							)
						}),
					)
				}

				log.info("Discovery complete", {
					server: activeServer.name,
					url,
					projects: projects.length,
					prefetched: sortedByActivity.length,
				})
			} catch (err) {
				log.error("Discovery failed", err)
				discoveryInFlight = false
				appStore.set(discoveryAtom, (prev) => ({
					...prev,
					loading: false,
					error: err instanceof Error ? err.message : "Discovery failed",
					phase: "error",
				}))
			}
		})()
	}, [loaded, loading, isMockMode, activeServer])
}
