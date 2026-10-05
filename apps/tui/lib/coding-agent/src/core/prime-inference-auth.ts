export const PRIME_INFERENCE_PROVIDER_ID = "prime-inference";
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
