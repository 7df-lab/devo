import { useAtomValue, useSetAtom } from "jotai"
import { useCallback } from "react"
import {
	agentFamily,
	agentsAtom,
	formatElapsed,
	formatRelativeTime,
	projectListAtom,
	sessionNameFamily,
} from "../atoms/derived/agents"
import { type DisplayMode, displayModeAtom } from "../atoms/preferences"
import { commandPaletteOpenAtom } from "../atoms/ui"
import { persistAppearanceSettings } from "../lib/settings-sync"
import type { Agent, SidebarProject } from "../lib/types"

// Re-export helpers from derived atom module
export { formatRelativeTime, formatElapsed }

/**
 * Hook that returns agents derived from live server sessions + discovered sessions.
 */
export function useAgents(): Agent[] {
	return useAtomValue(agentsAtom)
}

/**
 * Hook that returns the Agent for a single session ID.
 * Only subscribes to that session's data, not all sessions.
 */
export function useAgent(sessionId: string): Agent | null {
	return useAtomValue(agentFamily(sessionId))
}

/**
 * Hook that returns just the session title for a given session ID.
 * Used for parent session name lookups without subscribing to all agents.
 */
export function useSessionName(sessionId: string): string | undefined {
	return useAtomValue(sessionNameFamily(sessionId))
}

/**
 * Hook that returns the project list for the sidebar.
 */
export function useProjectList(): SidebarProject[] {
	return useAtomValue(projectListAtom)
}

/**
 * Individual UI selectors — thin wrappers around Jotai atoms.
 */
export const useCommandPaletteOpen = () => useAtomValue(commandPaletteOpenAtom)
export const useSetCommandPaletteOpen = () => useSetAtom(commandPaletteOpenAtom)
export const useDisplayMode = (): DisplayMode => useAtomValue(displayModeAtom)
export const useSetDisplayMode = () => {
	const setDisplayMode = useSetAtom(displayModeAtom)
	return useCallback(
		(displayMode: DisplayMode) => {
			setDisplayMode(displayMode)
			void persistAppearanceSettings({ displayMode })
		},
		[setDisplayMode],
	)
}
