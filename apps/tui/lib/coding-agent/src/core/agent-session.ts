export type RlmChildAgentStatus = "queued" | "running" | "done" | "error" | "cancelled";
export interface RlmChildAgentActivity { status?: RlmChildAgentStatus }
export interface RlmChildAgentSnapshot { id?: string; status?: RlmChildAgentStatus }
export type CompactionReason = "manual" | "threshold" | "overflow" | "requested";
export type AgentSessionEvent = { type: string; [key: string]: unknown };
export type AgentSessionEventListener = (event: AgentSessionEvent) => void;
export class CompactionSkippedError extends Error {}
export class RefineSkippedError extends Error {}
export interface AgentSessionConfig { [key: string]: unknown }
export interface ExtensionBindings { [key: string]: unknown }
export interface AutoRefineReviewRequest { [key: string]: unknown }
export type SerializedBackgroundPlanResult = { [key: string]: unknown };
export type AutoRefineReviewer = (...args: unknown[]) => Promise<unknown>;
export interface PromptOptions { [key: string]: unknown }
export interface TurnExecutionPolicy { [key: string]: unknown }
export const SESSION_ACTION_RECOVERY_FORMAT_VERSION = 1;
export interface SessionActionRecoveryRecord { [key: string]: unknown }
export type SessionActionRecoveryPayload = { [key: string]: unknown };
export interface SessionActionRecoveryAction { [key: string]: unknown }
export interface SessionActionRecoverySnapshot { [key: string]: unknown }
export interface ModelCycleResult { [key: string]: unknown }
export function compactRlmText(text: string, maxLength = 160): string {
	return text.length <= maxLength ? text : text.slice(0, maxLength);
}
export function rlmChildLabel(prompt: string): string { return prompt.slice(0, 80); }
export class AgentSession {
	settingsManager: unknown;
	modelRegistry: unknown;
	sessionManager: { getCwd(): string; getSessionName(): string | undefined } = {
		getCwd: () => process.cwd(),
		getSessionName: () => undefined,
	};
	resourceLoader: { getThemes(): { themes: unknown[] } } = { getThemes: () => ({ themes: [] }) };
	extensionRunner: unknown;
	systemPrompt = "";
	agent = { signal: undefined as AbortSignal | undefined };
	refreshMcpProviders(): void {}
	getToolDefinition(_name: string): undefined { return undefined; }
	bindExtensions(_bindings: unknown): Promise<void> { return Promise.resolve(); }
}
