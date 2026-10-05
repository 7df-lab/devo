export type AgentTracePreviewResult = { ok: false };
export type AgentTraceUploadResult = { ok: false };
export type AgentTraceUploadAllResult = { uploaded: number };
export async function previewAgentTraceFile(): Promise<AgentTracePreviewResult> { return { ok: false }; }
export async function findAgentTraceFiles(): Promise<string[]> { return []; }
export async function uploadAllAgentTraces(): Promise<AgentTraceUploadAllResult> { return { uploaded: 0 }; }
export function uploadAgentTraceFile(): Promise<AgentTraceUploadResult> { return Promise.resolve({ ok: false }); }
export function installAgentTraceUpload(): void {}
export function getPrimeAgentTraceCredential(): undefined { return undefined; }
