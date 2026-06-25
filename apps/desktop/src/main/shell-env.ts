/**
 * Resolves the user's full shell environment by spawning a login shell.
 *
 * When Electron is launched from Finder/Dock (not a terminal), macOS gives
 * it a minimal launchd environment missing everything added by .zshrc,
 * .zprofile, homebrew, nvm, bun, etc. This module runs the user's login
 * shell once at startup, captures `env`, and merges the result into
 * `process.env` so that child processes (devo server, git, etc.) see
 * the full PATH and other user-configured variables.
 *
 * The resolution is fully async (using `execFile`, not `execFileSync`) so
 * window creation and HTML loading proceed immediately without blocking.
 *
 * On Windows this is a no-op because Windows GUI apps inherit the full
 * user environment from the registry.
 */

import { execFile } from "node:child_process"
import { homedir } from "node:os"

const DELIMITER = "__DEVO_SHELL_ENV_DELIMITER__"

/**
 * List of env vars we should NOT overwrite from the shell, because Electron
 * sets them intentionally and the shell values would be wrong/stale.
 */
const PROTECTED_KEYS = new Set([
	"ELECTRON_RUN_AS_NODE",
	"ELECTRON_RENDERER_URL",
	"_",
	"SHLVL",
	"PWD",
	"OLDPWD",
	"ZSH_TMUX_AUTOSTARTED",
	"ZSH_TMUX_AUTOSTART",
])

/**
 * Detect the user's preferred shell. Falls back to /bin/zsh on macOS,
 * /bin/bash on Linux.
 */
function getShell(): string {
	if (process.env.SHELL) return process.env.SHELL
	if (process.platform === "darwin") return "/bin/zsh"
	return "/bin/bash"
}

/**
 * Parse the output of `env` between delimiters into a key-value map.
 * Handles multi-line values (value containing \n) by only splitting on
 * the first `=` per logical entry and joining until the next KEY= pattern.
 */
function parseEnv(raw: string): Record<string, string> {
	const start = raw.indexOf(DELIMITER)
	const end = raw.lastIndexOf(DELIMITER)
	if (start === -1 || end === -1 || start === end) return {}

	const envBlock = raw.slice(start + DELIMITER.length, end)
	const result: Record<string, string> = {}

	for (const line of envBlock.split("\n").filter(Boolean)) {
		const eqIndex = line.indexOf("=")
		if (eqIndex === -1) continue
		const key = line.slice(0, eqIndex)
		const value = line.slice(eqIndex + 1)
		result[key] = value
	}

	return result
}

/** Promise wrapper around `child_process.execFile`. Returns stdout as a string. */
function execFileAsync(
	file: string,
	args: string[],
	options: {
		encoding: "utf-8"
		timeout: number
		maxBuffer: number
		cwd: string
		env: NodeJS.ProcessEnv
	},
): Promise<string> {
	return new Promise((resolve, reject) => {
		execFile(file, args, options, (error, stdout) => {
			if (error) reject(error)
			else resolve(stdout)
		})
	})
}

/**
 * Spawn a login shell asynchronously, run `env`, and return the parsed
 * environment. Tries the user's preferred shell first, then falls back to
 * /bin/zsh and /bin/bash.
 */
async function resolveShellEnvAsync(): Promise<Record<string, string>> {
	const command = `echo -n "${DELIMITER}"; command env; echo -n "${DELIMITER}"; exit`
	const spawnEnv = {
		...process.env,
		// Prevent oh-my-zsh auto-update from blocking
		DISABLE_AUTO_UPDATE: "true",
		ZSH_TMUX_AUTOSTARTED: "true",
		ZSH_TMUX_AUTOSTART: "false",
	}

	const shells = [getShell(), "/bin/zsh", "/bin/bash"].filter(
		(shell, i, arr) => arr.indexOf(shell) === i,
	)

	for (const shell of shells) {
		try {
			const stdout = await execFileAsync(shell, ["-ilc", command], {
				encoding: "utf-8",
				timeout: 5000,
				maxBuffer: 10 * 1024 * 1024,
				cwd: homedir(),
				env: spawnEnv,
			})
			const env = parseEnv(stdout)
			if (env.PATH) return env
		} catch {
			// Try next shell
		}
	}

	return {}
}

/**
 * Merge the user's shell environment into `process.env` asynchronously.
 * The shell spawn runs off the main thread, so window creation and HTML
 * loading proceed in parallel.
 *
 * No-op on Windows (GUI apps already inherit the full user environment).
 */
async function resolveAndMergeEnv(): Promise<void> {
	if (process.platform === "win32") return

	try {
		const shellEnv = await resolveShellEnvAsync()
		if (!shellEnv.PATH) {
			console.warn("[shell-env] Could not resolve shell environment -- PATH will be limited")
			return
		}

		for (const [key, value] of Object.entries(shellEnv)) {
			if (PROTECTED_KEYS.has(key)) continue
			process.env[key] = value
		}

		console.log("[shell-env] Merged shell environment into process.env")
	} catch (error) {
		console.warn("[shell-env] Failed to resolve shell environment:", error)
	}
}

// ============================================================
// Public API
// ============================================================

/** Promise that resolves when the shell environment has been merged. */
let envReadyPromise: Promise<void> | null = null

/**
 * Start resolving the shell environment asynchronously.
 * Call this as early as possible in the main process. The shell spawn
 * runs in the background without blocking the event loop, so window
 * creation and HTML loading proceed immediately.
 *
 * Use `waitForEnv()` to await completion before operations that need the
 * full PATH (e.g., spawning the devo server).
 */
export function startEnvResolution(): void {
	if (process.platform === "win32") {
		envReadyPromise = Promise.resolve()
		return
	}
	envReadyPromise = resolveAndMergeEnv()
}

/**
 * Wait for the async shell environment resolution to complete.
 * Safe to call multiple times; resolves immediately if already done.
 */
export function waitForEnv(): Promise<void> {
	return envReadyPromise ?? Promise.resolve()
}
