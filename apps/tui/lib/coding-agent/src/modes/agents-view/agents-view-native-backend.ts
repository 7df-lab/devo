/**
 * Native (non-daemon) backend for AgentsViewMode.
 *
 * When `AgentsViewModeOptions.nativeBackend` is set, the view uses this surface
 * instead of DaemonClient / AgentsViewRosterStore. Devo's TUI implements the
 * adapter over Native `session/*` + `agent/list` + a shared AgentConnection.
 */

import type { AgentConnection, AgentConnectionHeartbeat } from "../agent-connection/types.js";
import type { SessionSummary } from "../daemon/daemon-session-list.js";

/** Live roster feed with the same consumer shape AgentsViewMode expects from RosterStore. */
export interface AgentsViewNativeRoster {
	summaries(): SessionSummary[];
	onUpdate(listener: () => void): () => void;
	refresh(): Promise<void>;
	dispose(): Promise<void>;
}

export interface AgentsViewNativeOpenedSession {
	connection: AgentConnection;
	summary: SessionSummary;
	cwdFallbackNotice?: string;
}

/**
 * Host-owned Native operations for Agents View.
 * Missing optional methods degrade to a status message in the view.
 */
export interface AgentsViewNativeBackend {
	createRoster(): Promise<AgentsViewNativeRoster>;
	openSession(summary: SessionSummary): Promise<AgentsViewNativeOpenedSession>;
	createSession(): Promise<AgentsViewNativeOpenedSession>;
	renameSession?(summary: SessionSummary, name: string): Promise<void>;
	/** Soft-stop a live runtime (Prime `kill`); session remains listable/resumable. */
	killSession?(summary: SessionSummary): Promise<void>;
	/** Delete or trash a session permanently. */
	deleteSession?(summary: SessionSummary): Promise<void>;
	cancelSubagent?(rootActiveSessionId: string, childId: string): Promise<void>;
	getLastAssistantText?(activeSessionId: string): Promise<string | undefined>;
	listHeartbeats?(): Promise<AgentConnectionHeartbeat[]>;
	prompt?(
		sessionId: string,
		message: string,
		streamingBehavior?: "steer" | "followUp",
	): Promise<void>;
}
