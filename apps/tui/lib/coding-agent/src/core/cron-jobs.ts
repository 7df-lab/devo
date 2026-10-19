export type AgentCronJobStatus = "active" | "paused" | "completed" | "cancelled";
export type AgentCronScheduleKind = "once" | "cron" | "interval";
export type AgentCronJobSource = "cron" | "heartbeat" | "rlm_heartbeat";
export type AgentCronJobRuntimeKind = "top-level" | "subagent";
export type AgentHeartbeatUpdateAction = "pause" | "resume" | "clear";
export type AgentHeartbeatManagementAction = "pause" | "resume" | "stop";
export type AgentRlmHeartbeatStatusUpdate = "pause" | "resume";
export type AgentHeartbeatDeliveryMode = "steer" | "follow_up";
export const DEFAULT_HEARTBEAT_DELIVERY_MODE: AgentHeartbeatDeliveryMode = "steer";
export interface AgentCronSchedule { kind: AgentCronScheduleKind; expression: string; intervalMs?: number }
export interface AgentCronJob {
	id: string;
	status: AgentCronJobStatus;
	source?: AgentCronJobSource;
	runtimeKind?: AgentCronJobRuntimeKind;
	deliveryMode?: AgentHeartbeatDeliveryMode;
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
export interface HeartbeatCommandResult {
	action: "status" | "set" | "pause" | "resume" | "clear";
	job?: AgentCronJob;
}
export function isHeartbeatCronJob(job: AgentCronJob): boolean {
	return job.source === "heartbeat" || job.source === "rlm_heartbeat";
}
