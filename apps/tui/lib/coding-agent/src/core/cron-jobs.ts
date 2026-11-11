export type AgentCronJobStatus = "active" | "paused" | "completed" | "cancelled";
export type AgentCronScheduleKind = "once" | "cron" | "interval";
export type AgentCronJobSource = "cron" | "heartbeat" | "rlm_heartbeat";
export type AgentCronJobRuntimeKind = "top-level" | "subagent";
export type AgentScheduledUpdateAction = "pause" | "resume" | "clear";
export type AgentScheduledManagementAction = "pause" | "resume" | "stop";
export type AgentRlmScheduledStatusUpdate = "pause" | "resume";
export type AgentScheduledDeliveryMode = "steer" | "follow_up";
export const DEFAULT_SCHEDULED_DELIVERY_MODE: AgentScheduledDeliveryMode = "steer";
export interface AgentCronSchedule { kind: AgentCronScheduleKind; expression: string; intervalMs?: number }
export interface AgentCronJob {
	id: string;
	status: AgentCronJobStatus;
	source?: AgentCronJobSource;
	runtimeKind?: AgentCronJobRuntimeKind;
	deliveryMode?: AgentScheduledDeliveryMode;
	activeSessionId: string;
	sessionId: string;
	sessionFile: string;
	cwd: string;
	label?: string;
	prompt: string;
	schedule: AgentCronSchedule;
	createdAt?: string;
	updatedAt?: string;
	nextRunAt?: string;
	lastRunAt?: string;
	runCount?: number;
}
/** Result of the native `session/heartbeat/command` RPC, surfaced by the connection layer. */
export interface ScheduledCommandResult {
	action: "status" | "set" | "pause" | "resume" | "clear";
	job?: AgentCronJob;
}
export function isScheduledCronJob(job: AgentCronJob): boolean {
	return job.source === "heartbeat" || job.source === "rlm_heartbeat";
}
