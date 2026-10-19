export type AgentExecutionMode = "interactive";
export interface AgentSessionRuntimeConfig {
	cwd?: string;
	agentDir?: string;
	sessionDir?: string;
	telemetryDisabled?: true;
}
export type DurableAgentSessionRuntimeConfig = Pick<AgentSessionRuntimeConfig, "cwd" | "agentDir" | "sessionDir" | "telemetryDisabled">;
export function durableAgentSessionRuntimeConfig(config: AgentSessionRuntimeConfig): DurableAgentSessionRuntimeConfig {
	return { cwd: config.cwd, agentDir: config.agentDir, sessionDir: config.sessionDir, telemetryDisabled: config.telemetryDisabled };
}
