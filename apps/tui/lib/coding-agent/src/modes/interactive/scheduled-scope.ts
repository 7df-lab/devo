import type { AgentConnectionScheduledTask, AgentConnectionRlmChildAgentSnapshot } from "../agent-connection/types.js";

interface ScheduledSessionIdentity {
	activeSessionId?: string;
	sessionId: string;
}

export function scopeScheduledTasksToSession(
	scheduledTasks: readonly AgentConnectionScheduledTask[],
	session: ScheduledSessionIdentity | undefined,
	children: Iterable<Pick<AgentConnectionRlmChildAgentSnapshot, "activeSessionId">>,
): AgentConnectionScheduledTask[] {
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

	return scheduledTasks.filter(
		(scheduled) =>
			scheduled.job.sessionId === session.sessionId || activeSessionIds.has(scheduled.job.activeSessionId),
	);
}
