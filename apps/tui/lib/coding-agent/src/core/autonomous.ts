export interface AgentAutonomousConfig { enabled?: boolean }
export interface AgentAutonomousStatus { enabled: boolean }
export type AutonomousLimitReason = string;
export function autonomousLimitReason(_status: AgentAutonomousStatus): string | undefined { return undefined; }
