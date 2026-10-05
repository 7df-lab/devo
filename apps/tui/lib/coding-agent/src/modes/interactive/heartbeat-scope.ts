import type { AgentConnectionHeartbeat, AgentConnectionRlmChildAgentSnapshot } from "../agent-connection/types.js";

interface HeartbeatSessionIdentity {
	activeSessionId?: string;
	sessionId: string;
}

export function scopeHeartbeatsToSession(
	heartbeats: readonly AgentConnectionHeartbeat[],
	session: HeartbeatSessionIdentity | undefined,
	children: Iterable<Pick<AgentConnectionRlmChildAgentSnapshot, "activeSessionId">>,
): AgentConnectionHeartbeat[] {
	if (!session) {
		return [];
	}

	const activeSessionIds = new Set<string>();
	if (session.activeSessionId) {
		activeSessionIds.add(session.activeSessionId);
	}
	for (const child of children) {
		if (child.activeSessionId) {
			activeSessionIds.add(child.activeSessionId);
		}
	}

	return heartbeats.filter(
		(heartbeat) =>
			heartbeat.job.sessionId === session.sessionId || activeSessionIds.has(heartbeat.job.activeSessionId),
	);
}
