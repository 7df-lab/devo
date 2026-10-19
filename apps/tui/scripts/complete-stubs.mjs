/**
 * Complete thin stubs so Devo InteractiveMode can load without upstream
 * daemon/kernel/AgentSession implementations.
 *
 * Policy (Devo Native path):
 * - Do NOT resurrect upstream daemon / ACP / in-process AgentSession as product.
 * - Prefer DELETE unused stubs; KEEP thin no-op/throw stubs only while still imported.
 * - Node ReplKernelManager must fail closed — ipython is Rust Native.
 *
 * Removed (unused): daemon-mode, daemon-supervisor, modes/acp,
 * DaemonAgentConnection, InProcessAgentConnection, DaemonClient class.
 * Removed (Rust Native owns kernels): kernel/repl-manager.ts, kernel/bootstrap.ts,
 * kernel/boot-gate.ts, kernel/state-snapshot.ts(+test), core/rlm-runtime.ts;
 * core/kernel/ now only re-exports shared.ts types.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const ca = path.join(repo, "apps/tui/lib/coding-agent/src");

// Non-destructive by default: a fresh vendor drop has no stub files yet, so
// plain `node scripts/complete-stubs.mjs` after a drop writes them all. On a
// tree where the stubs already exist (possibly hand-adapted afterwards), the
// script must NOT clobber them — pass --force to overwrite explicitly.
const force = process.argv.includes("--force");

function write(rel, contents) {
  const p = path.join(ca, rel);
  if (!force && fs.existsSync(p)) {
    console.log(`skip (exists; --force to overwrite): ${rel}`);
    return;
  }
  fs.mkdirSync(path.dirname(p), { recursive: true });
  fs.writeFileSync(p, contents.endsWith("\n") ? contents : `${contents}\n`);
}

write(
  "core/prime-inference-models.ts",
  `export function isPrivatePrimeInferenceModel(_model: { provider?: string; id?: string }): boolean { return false; }
export function getPrivatePrimeInferenceModels(): never[] { return []; }
export async function fetchAuthorizedPrivatePrimeInferenceModels(..._args: unknown[]): Promise<never[]> { return []; }
export const PRIME_INFERENCE_DEFAULT_MODEL_ID = "";
export const PRIME_INFERENCE_BASE_URL = "";
`,
);

write(
  "core/prime-inference-model-catalog.ts",
  `export const PRIME_INFERENCE_PROVIDER_ID = "prime-inference";
export const PRIME_INFERENCE_BASE_URL = "";
export function buildPrimeInferenceModels<T>(..._args: unknown[]): T[] { return []; }
export function mergePrimeInferenceModels<T>(models: T[] = []): T[] { return models; }
export function readCachedPrimeInferenceModels(): unknown[] { return []; }
export class PrimeInferenceCatalogRequestError extends Error {}
export async function fetchPrimeInferenceModelCatalog(): Promise<unknown[]> { return []; }
export async function refreshPrimeInferenceModels(): Promise<void> {}
`,
);

write(
  "core/prime-inference-model-selection.ts",
  `export function resolvePrimeInferencePostLoginModelAction(..._args: unknown[]): undefined { return undefined; }
`,
);

write(
  "core/prime-inference-auth.ts",
  `export const PRIME_INFERENCE_PROVIDER_ID = "prime-inference";
export const PRIME_INFERENCE_PROVIDER_NAME = "Hosted Inference";
export const DEVO_TRACES_PROVIDER_ID = "prime-agent-traces";
export const DEVO_TRACES_PROVIDER_NAME = "Devo Traces";
export type PrimeInferenceAuthSource = "prime-cli" | "browser";
export type PrimeInferenceLoginResult = { ok: boolean; [key: string]: unknown };
export type PrimeInferenceLoginCallbacks = { [key: string]: unknown };
export type PrimeInferenceLoginOptions = { [key: string]: unknown };
export type PrimeInferenceAccessResult = { ok: boolean };
export type PrimeTeam = { id: string; name?: string };
export type PrimeChallengeConfig = { [key: string]: unknown };
export function getPrimeCliConfigPath(): string { return ""; }
export function resolvePrimeInferenceAuthConfig(): PrimeChallengeConfig { return {}; }
export function resolvePrimeAgentTracesBaseUrl(baseUrl?: string): string { return baseUrl ?? ""; }
export async function fetchPrimeTeams(): Promise<PrimeTeam[]> { return []; }
export async function checkPrimeInferenceAccess(): Promise<PrimeInferenceAccessResult> { return { ok: false }; }
export async function checkPrimeAgentTracesAccess(): Promise<PrimeInferenceAccessResult> { return { ok: false }; }
export async function loginPrimeInference(): Promise<never> { throw new Error("This provider login is not available in Devo"); }
export async function loginPrimeAgentTraces(): Promise<never> { throw new Error("Trace upload is not available in Devo"); }
export function getPrimeAgentTraceCredential(): undefined { return undefined; }
`,
);

write(
  "core/agent-session.ts",
  `export type RlmChildAgentStatus = "queued" | "running" | "done" | "error" | "cancelled";
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
`,
);

write(
  "core/agent-session-runtime.ts",
  `import type { AgentSession } from "./agent-session.js";
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
`,
);

write(
  "core/agent-session-services.ts",
  `export interface AgentSessionRuntimeDiagnostic { [key: string]: unknown }
export interface CreateAgentSessionServicesOptions { [key: string]: unknown }
export interface AgentSessionCreationOptions { [key: string]: unknown }
export interface CreateAgentSessionFromServicesOptions extends AgentSessionCreationOptions { [key: string]: unknown }
export interface AgentSessionServices {
	settingsManager: unknown;
	modelRegistry: unknown;
	resourceLoader: { getThemes(): { themes: unknown[] } };
	mcpManager: { refresh(): void };
}
export async function createAgentSessionServices(): Promise<AgentSessionServices> {
	return {
		settingsManager: undefined,
		modelRegistry: undefined,
		resourceLoader: { getThemes: () => ({ themes: [] }) },
		mcpManager: { refresh() {} },
	};
}
export async function createAgentSessionFromServices(): Promise<{ session: undefined }> {
	return { session: undefined };
}
`,
);

write(
  "core/agent-session-config.ts",
  `export type AgentExecutionMode = "interactive";
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
`,
);

write(
  "core/package-manager.ts",
  `export interface PathMetadata {
	source: string;
	scope?: string;
	origin?: "package" | "top-level";
	baseDir?: string;
}
export interface ResolvedResource {
	path: string;
	enabled: boolean;
	metadata: PathMetadata;
}
export interface ResourceDiagnostic { [key: string]: unknown }
export interface ResolvedPaths {
	extensions: ResolvedResource[];
	skills: ResolvedResource[];
	prompts: ResolvedResource[];
	themes: ResolvedResource[];
	diagnostics: ResourceDiagnostic[];
}
export type MissingSourceAction = "install" | "skip" | "error";
export interface ProgressEvent { [key: string]: unknown }
export type ProgressCallback = (event: ProgressEvent) => void;
export interface PackageUpdate { source?: string; displayName: string; type?: string; scope?: string }
export interface ConfiguredPackage { [key: string]: unknown }
export interface PackageManager {
	resolve(): Promise<ResolvedPaths>;
}
const emptyResolved = (): ResolvedPaths => ({
	extensions: [],
	skills: [],
	prompts: [],
	themes: [],
	diagnostics: [],
});
export class DefaultPackageManager implements PackageManager {
	constructor(_options?: unknown) {}
	async checkForAvailableUpdates(): Promise<PackageUpdate[]> { return []; }
	async resolve(): Promise<ResolvedPaths> { return emptyResolved(); }
	async resolveExtensionSources(_paths?: unknown, _options?: unknown): Promise<ResolvedPaths> { return emptyResolved(); }
	async install(): Promise<void> {}
	async installAndPersist(): Promise<void> {}
}
`,
);

write(
  "core/telemetry.ts",
  `export type TelemetryEventName = string;
export type TelemetryExecutionMode = string;
export type TelemetryOnboardingOutcome = "success" | "error" | "aborted";
export type TelemetryAuthCategory = string;
export interface TelemetryEvent { [key: string]: unknown }
export interface TelemetryBatch { [key: string]: unknown }
export interface TelemetrySink { [key: string]: unknown }
export interface CaptureOnboardingCompletedOptions { [key: string]: unknown }
export interface CaptureAgentCommandUsedOptions { [key: string]: unknown }
export function isTelemetryEnabled(_settingsManager?: unknown): boolean { return false; }
export function getOrCreateTelemetryInstallationId(_agentDir?: string): string { return "devo"; }
export class TelemetryClient {}
export function telemetryProviderCategory(provider?: string): string { return provider ?? "unknown"; }
export function telemetryAuthCategory(_provider?: string): string { return "unknown"; }
export async function captureOnboardingCompleted(_options?: unknown): Promise<void> {}
export async function captureAgentCommandUsed(_options?: unknown): Promise<void> {}
export function installAgentTelemetry(_session?: unknown, _options?: unknown): void {}
`,
);

write(
  "core/agent-traces.ts",
  `export type AgentTracePreviewResult = { ok: false };
export type AgentTraceUploadResult = { ok: false };
export type AgentTraceUploadAllResult = { uploaded: number };
export async function previewAgentTraceFile(): Promise<AgentTracePreviewResult> { return { ok: false }; }
export async function findAgentTraceFiles(): Promise<string[]> { return []; }
export async function uploadAllAgentTraces(): Promise<AgentTraceUploadAllResult> { return { uploaded: 0 }; }
export function uploadAgentTraceFile(): Promise<AgentTraceUploadResult> { return Promise.resolve({ ok: false }); }
export function installAgentTraceUpload(): void {}
export function getPrimeAgentTraceCredential(): undefined { return undefined; }
`,
);

write(
  "core/autonomous.ts",
  `export interface AgentAutonomousConfig { enabled?: boolean }
export interface AgentAutonomousStatus { enabled: boolean }
export type AutonomousLimitReason = string;
export function autonomousLimitReason(_status: AgentAutonomousStatus): string | undefined { return undefined; }
`,
);

write(
  "core/cron-jobs.ts",
  `export type AgentCronJobStatus = "active" | "paused" | "completed" | "cancelled";
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
/** Result of the native \`session/heartbeat/command\` RPC, surfaced by the connection layer. */
export interface HeartbeatCommandResult {
	action: "status" | "set" | "pause" | "resume" | "clear";
	job?: AgentCronJob;
}
export function isHeartbeatCronJob(job: AgentCronJob): boolean {
	return job.source === "heartbeat" || job.source === "rlm_heartbeat";
}
`,
);

write(
  "utils/version-check.ts",
  `export async function checkForNewPiVersion(..._args: unknown[]): Promise<string | undefined> { return undefined; }
export async function getLatestPiRelease(..._args: unknown[]): Promise<undefined> { return undefined; }
export function isNewerPackageVersion(): boolean { return false; }
`,
);

write(
  "cli/args.ts",
  `export function isValidThinkingLevel(value: unknown): boolean { return typeof value === "string"; }
`,
);

write(
  "cli/daemon-update-restart.ts",
  `export function buildDaemonUpdateRestartReport(): string { return ""; }
export function launchDaemonUpdateRestartCoordinator(): never {
	throw new Error("Devo TUI does not self-update");
}
export function resolveDaemonUpdateRestartSocketPath(): undefined { return undefined; }
`,
);

write(
  "cli/subprocess-launch.ts",
  `export interface CliSubprocessLaunchSpec { [key: string]: unknown }
export function createCliSubprocessLaunchSpec(): CliSubprocessLaunchSpec { return {}; }
export function createUpdatedCliSubprocessLaunchSpec(): CliSubprocessLaunchSpec { return {}; }
`,
);

write(
  "modes/daemon/daemon-client.ts",
  `/** Types for legacy Agents View daemon catalogs. Devo product uses Native backends. */

export type DaemonCommandBody = { type: string; [key: string]: unknown };
export type DaemonHello = { type: "daemon_hello"; [key: string]: unknown };
export type DaemonClientMessageListener = (message: unknown) => void;
export type DaemonClientCloseListener = (error: Error) => void;
export type DaemonClientProgressListener = (message: unknown) => void;
export interface DaemonClientRequestOptions {
	[key: string]: unknown;
}
export class DaemonSocketClosedError extends Error {}
export class DaemonCapabilityUnavailableError extends Error {}
export function getDaemonSocketCloseReason(_error: Error): string | undefined {
	return undefined;
}
export type DaemonClientReconnectStatus = string;
export interface DaemonClientReconnectOptions {
	[key: string]: unknown;
}

/** Minimal client surface used by optional daemon roster/catalog helpers. */
export interface DaemonTransportClient {
	request(command: unknown, options?: unknown): Promise<unknown>;
	onMessage?(listener: DaemonClientMessageListener): void;
	close?(): void;
	readonly isConnected?: boolean;
	readonly hello?: DaemonHello;
	waitForHello?(): Promise<DaemonHello | undefined>;
	supportsServerCapability?(name: string): boolean;
}
`,
);

console.log("complete stubs written");
