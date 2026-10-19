import type { AgentSession } from "./agent-session.js";
export { SessionImportFileNotFoundError } from "./session-import-errors.js";
export interface CreateAgentSessionRuntimeResult { session?: AgentSession; [key: string]: unknown }
export type CreateAgentSessionRuntimeFactory = (options: unknown) => Promise<CreateAgentSessionRuntimeResult>;
export type AgentSessionRuntimeKind = "top-level" | "subagent";
export interface AgentSessionRuntimeMetadata { [key: string]: unknown }
export interface AgentSessionRuntimeDisposeOptions { [key: string]: unknown }
export class AgentSessionRuntime {
	session!: AgentSession;
	newSession(_options?: unknown): Promise<{ cancelled: boolean }> { return Promise.resolve({ cancelled: true }); }
	fork(_entryId: string, _options?: unknown): Promise<{ cancelled: boolean }> { return Promise.resolve({ cancelled: true }); }
	switchSession(_path: string, _options?: unknown): Promise<{ cancelled: boolean }> { return Promise.resolve({ cancelled: true }); }
}
export async function createAgentSessionRuntime(): Promise<CreateAgentSessionRuntimeResult> {
	return {};
}
