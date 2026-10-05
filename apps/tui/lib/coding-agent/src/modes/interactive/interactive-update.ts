import { APP_NAME } from "../../config.js";
import type { CliSubprocessLaunchSpec } from "../../cli/subprocess-launch.js";

export function updateArgsIncludeSelf(args: readonly string[]): boolean {
	let selfFlag = false;
	let extensionsOnlyFlag = false;
	let positional: string | undefined;
	for (let index = 0; index < args.length; index++) {
		const arg = args[index];
		if (arg === "--self") {
			selfFlag = true;
		} else if (arg === "--extensions") {
			extensionsOnlyFlag = true;
		} else if (arg === "--extension") {
			extensionsOnlyFlag = true;
			index++;
		} else if (arg === "--daemon-socket") {
			index++;
		} else if (arg && !arg.startsWith("-") && positional === undefined) {
			positional = arg;
		}
	}
	if (selfFlag) {
		return true;
	}
	if (extensionsOnlyFlag) {
		return false;
	}
	if (!positional) {
		return true;
	}
	const normalized = positional.toLowerCase();
	return normalized === "self" || normalized === "pi" || normalized === APP_NAME.toLowerCase();
}

function argsIncludeSessionSelection(args: readonly string[]): boolean {
	for (const arg of args) {
		if (arg === "--resume" || arg === "-r" || arg === "--continue" || arg === "-c" || arg === "--fork") {
			return true;
		}
	}
	return false;
}

export function buildUpdateRelaunchArgs(args: readonly string[], sessionFile: string | undefined): string[] {
	const relaunchArgs = [...args];
	if (sessionFile && !argsIncludeSessionSelection(relaunchArgs)) {
		relaunchArgs.push("--resume", sessionFile);
	}
	return relaunchArgs;
}

type UpdateRelaunchExecve = (file: string, args: string[], environment: Record<string, string>) => never;

interface UpdateRelaunchExecOptions {
	platform: string;
	nodeVersion: string;
	cwd: string;
	previousCwd: string;
	environment: NodeJS.ProcessEnv;
	chdir: (directory: string) => void;
	execve?: UpdateRelaunchExecve;
}

function execveFailureThrows(nodeVersion: string): boolean {
	// Before Node 26.1, a failed execve syscall aborts the process instead of throwing for the fallback below.
	const match = /^(\d+)\.(\d+)\./.exec(nodeVersion);
	if (!match) {
		return false;
	}
	const major = Number(match[1]);
	const minor = Number(match[2]);
	return major > 26 || (major === 26 && minor >= 1);
}

export function tryExecUpdateRelaunch(launch: CliSubprocessLaunchSpec, options: UpdateRelaunchExecOptions): boolean {
	// Process replacement preserves the shell job and foreground terminal without retaining the old TUI.
	if (
		!options.execve ||
		options.platform === "win32" ||
		options.platform === "os400" ||
		!execveFailureThrows(options.nodeVersion)
	) {
		return false;
	}
	const environment = Object.fromEntries(
		Object.entries(options.environment).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
	);
	options.chdir(options.cwd);
	try {
		options.execve(launch.command, [launch.command, ...launch.args], environment);
	} catch (error) {
		// A thrown execve must not leave the fallback's process on a changed cwd.
		options.chdir(options.previousCwd);
		throw error;
	}
	return true;
}

export function buildUpdateChildArgs(args: readonly string[], daemonSocketPath: string): string[] {
	return args.includes("--daemon-socket") ? [...args] : [...args, "--daemon-socket", daemonSocketPath];
}

export function resolveInteractiveUpdateDaemonSocketPath(
	args: readonly string[],
	activeDaemonSocketPath: string,
): string {
	const socketFlagIndex = args.indexOf("--daemon-socket");
	return socketFlagIndex === -1 ? activeDaemonSocketPath : (args[socketFlagIndex + 1] ?? activeDaemonSocketPath);
}
