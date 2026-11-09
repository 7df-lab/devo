/**
 * Reusable session view component.
 *
 * Renders the full chat UI (AgentDetail with ChatView, prompt input, app bar
 * integration, undo/redo, permissions, etc.) for any given sessionId.
 *
 * This is the extracted "controller" logic that was previously inlined in
 * SessionRoute. Both SessionRoute (for route-driven sessions) and
 * AutomationRunDetail (for automation sessions) use this component.
 */

import { useSessionController } from "../hooks/use-session-controller"
import { AgentDetail } from "./agent-detail"
import { SessionResolutionState } from "./session-resolution-state"

interface SessionViewProps {
	/** The Devo session ID to display */
	sessionId: string
	/** Whether this instance is the visible session (keep-alive panels pass false). */
	isActive?: boolean
}

export function SessionView({ sessionId, isActive = true }: SessionViewProps) {
	const controller = useSessionController(sessionId, isActive)

	if (!controller.agent) {
		if (!isActive) return null
		return (
			<SessionResolutionState state={controller.resolution} onRetry={controller.retryResolution} />
		)
	}

	return (
		<AgentDetail
			agent={controller.agent}
			chatTurns={controller.chatTurns}
			chatLoading={controller.chatLoading}
			chatShowLoading={controller.chatShowLoading}
			chatLoadingEarlier={controller.chatLoadingEarlier}
			chatHasEarlier={controller.chatHasEarlier}
			onLoadEarlier={controller.chatLoadEarlier}
			onStop={controller.handleStopAgent}
			onApprove={controller.handleApprovePermission}
			onDeny={controller.handleDenyPermission}
			onReplyQuestion={controller.handleReplyQuestion}
			onRejectQuestion={controller.handleRejectQuestion}
			onSendMessage={controller.handleSendMessage}
			onRename={controller.handleRenameSession}
			parentSessionName={controller.parentSessionName}
			isConnected={true}
			providers={controller.providers}
			config={controller.config}
			vcs={controller.vcs}
			devoAgents={controller.devoAgents}
			canUndo={controller.canUndo}
			canRedo={controller.canRedo}
			onUndo={controller.undo}
			onRedo={controller.redo}
			isReverted={controller.isReverted}
			onForkFromTurn={controller.handleForkFromTurn}
			onEditUserMessage={controller.handleEditUserMessage}
		/>
	)
}
