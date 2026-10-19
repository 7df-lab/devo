import { type Static, Type } from "typebox";
import type { ToolDefinition } from "../extensions/types.js";
import type { KernelAttachment, KernelDiffDisplay, KernelSentAgentMessage } from "../kernel/index.js";

const ipythonSchema = Type.Object({
	code: Type.String({
		description:
			"Python code to execute in the persistent Python REPL. Use the target project's own environment for project imports, tests, scripts, CLIs, and dependency checks instead of direct kernel imports.",
	}),
});

export type IpythonToolInput = Static<typeof ipythonSchema>;

export interface IpythonToolDetails {
	durationMs?: number;
	status?: "ok" | "error" | "aborted" | "starting";
	errorEname?: string;
	stdout?: string;
	stderr?: string;
	result?: string;
	/** Output that arrived without this cell's id (threads, other cells' leftovers), shown separately from stdout. */
	backgroundOutput?: string;
	/** Diffs streamed from file edits, rendered by the cell view. */
	diffs?: KernelDiffDisplay[];
	/** Media attachments loaded into context (e.g. by the attach-image skill). */
	attachments?: KernelAttachment[];
	/** Agent messages sent from this cell. */
	sentAgentMessages?: KernelSentAgentMessage[];
	/** True when this result came after killing and restarting a busy kernel. */
	kernelRestarted?: boolean;
	error?: {
		ename: string;
		evalue: string;
		traceback: string[];
	};
}

export interface IpythonToolOptions {
	/** Inert on the Devo Native path — execution lives in the Rust kernel host. */
	[key: string]: unknown;
}

export function createIpythonToolDefinition(
	_cwd: string,
	_options?: IpythonToolOptions,
): ToolDefinition<typeof ipythonSchema, IpythonToolDetails> {
	return {
		name: "ipython",
		label: "ipython",
		description:
			"Execute Python code in a persistent Python REPL. Top-level `await` is supported. Variables, imports, and loaded data persist across calls, and are revived on a best-effort basis when a session is resumed (objects that cannot be serialized are dropped and reported). Run shell commands with `bash('cmd')` / `await bash('cmd')`. Project imports, tests, scripts, CLIs, and dependency checks should run through the target project's own environment.",
		promptSnippet: "ipython - persistent Python REPL for code, state, and bash() orchestration",
		// The kernel is single-threaded — the Rust host serializes ipython calls.
		executionMode: "sequential",
		parameters: ipythonSchema,
		execute: async () => {
			throw new Error("ipython execution is not available in Devo TUI; it runs via the Native Rust kernel");
		},
	};
}
