/**
 * Kernel type surface shared with the TUI rendering layer.
 *
 * Execution lives in the Rust kernel host (`crates/kernel` + `crates/server`);
 * the Node ReplKernelManager and its runtime plumbing were removed. Only the
 * types and small helpers the UI imports remain.
 */

/**
 * Handles one typed request from Python code running in the kernel.
 * The returned record is delivered verbatim to the Python caller.
 */
export type HostRequestHandler = (payload: Record<string, unknown>) => Promise<Record<string, unknown>>;

/** Host request handlers keyed by request type (e.g. "rlm.run", "goal.complete"). */
export type HostRequestHandlers = Record<string, HostRequestHandler>;

/** Extra import labels advertised to the model alongside the Python skills (inert on the Native path). */
export const DEFAULT_RLM_EXTRA_IMPORT_LABELS: string[] = [];

/** One file edit, captured from a `application/vnd.devo.diff+json` display payload. */
export interface KernelDiffDisplay {
	path: string;
	oldStr: string;
	newStr: string;
	/** 1-based line where `oldStr` begins in the file, for absolute line numbers. */
	startLine?: number;
}

/** One media attachment, captured from an attachment display payload. */
export interface KernelAttachment {
	mimeType: string;
	/** base64-encoded bytes. */
	data: string;
	/** Source path, surfaced to the TUI renderer. */
	path?: string;
}

export interface KernelSentAgentMessage {
	id: string;
	message: string;
	deliveryStatus: "delivered" | "queued";
	receiverRole?: "parent" | "sibling" | "child";
	target: {
		activeSessionId: string;
		sessionId: string;
		sessionName?: string;
	};
}

export interface ExecuteResult {
	stdout: string;
	stderr: string;
	/** Text of the cell's trailing expression value, if the cell produced one. */
	result?: string;
	/** Diffs emitted via display events, in order. */
	diffs?: KernelDiffDisplay[];
	/** Media attachments emitted via display events, in order. */
	attachments?: KernelAttachment[];
	/** Agent messages sent from this cell, in order. */
	sentAgentMessages?: KernelSentAgentMessage[];
	/** Output that arrived without this cell's id (user threads, other cells' leftovers, raw fd writes). */
	backgroundOutput?: string;
	status: "ok" | "error" | "aborted";
	error?: { ename: string; evalue: string; traceback: string[] };
	durationMs: number;
}

export interface Deferred<T> {
	promise: Promise<T>;
	resolve: (value: T) => void;
	reject: (error: Error) => void;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

export function createDeferred<T>(): Deferred<T> {
	let resolve!: (value: T) => void;
	let reject!: (error: Error) => void;
	const promise = new Promise<T>((promiseResolve, promiseReject) => {
		resolve = promiseResolve;
		reject = promiseReject;
	});
	return { promise, resolve, reject };
}
