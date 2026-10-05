export type TelemetryEventName = string;
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
