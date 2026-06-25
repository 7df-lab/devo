import { useAtomValue } from "jotai"
import { useCallback, useEffect } from "react"
import { agentFamily } from "../atoms/derived/agents"
import { pendingCountAtom } from "../atoms/derived/waiting"
import { appStore } from "../atoms/store"

const isElectron = typeof window !== "undefined" && "devo" in window

/**
 * Handles native OS notification integration:
 * 1. Listens for notification clicks (main -> renderer) and navigates to the session
 * 2. Listens for tray New Chat requests and navigates to the start screen
 * 3. Syncs the pending count to the dock badge
 * 4. Auto-dismisses notifications when the user navigates to a session
 */
export function useNotifications(
	navigate: (opts: { to: string; params: Record<string, string> }) => void,
	currentSessionId: string | undefined,
) {
	// --- Badge sync ---
	const pendingCount = useAtomValue(pendingCountAtom)

	useEffect(() => {
		if (!isElectron) return
		window.devo.updateBadgeCount(pendingCount)
	}, [pendingCount])

	// --- Notification click -> navigate to session ---
	const handleNavigate = useCallback(
		(data: { sessionId: string }) => {
			// Find the agent to get its projectSlug
			const agent = appStore.get(agentFamily(data.sessionId))
			if (agent) {
				navigate({
					to: "/project/$projectSlug/session/$sessionId",
					params: {
						projectSlug: agent.projectSlug,
						sessionId: agent.id,
					},
				})
			}
		},
		[navigate],
	)

	useEffect(() => {
		if (!isElectron) return
		return window.devo.onNotificationNavigate(handleNavigate)
	}, [handleNavigate])

	useEffect(() => {
		if (!isElectron) return
		return window.devo.onTrayNewChat(() => navigate({ to: "/", params: {} }))
	}, [navigate])

	// --- Auto-dismiss when viewing a session ---
	useEffect(() => {
		if (!isElectron || !currentSessionId) return
		window.devo.dismissNotification(currentSessionId)
	}, [currentSessionId])
}
