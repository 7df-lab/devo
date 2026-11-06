/**
 * Devo runtime version compatibility definitions for Desktop.
 *
 * Updated with each Devo release to reflect tested Devo versions.
 * The environment check in the onboarding flow uses these ranges to
 * decide whether to pass, warn, or block.
 */

import { execFile } from "node:child_process"
import { coerce, satisfies, valid } from "semver"
import { createLogger } from "./logger"

const log = createLogger("compatibility")

// ============================================================
// Compatibility ranges (standard semver range syntax)
// ============================================================

export const DEVO_COMPAT = {
	/** Supported range -- versions that should work. Below this: hard block. */
	supported: ">=0.1.21",
	/** Tested range -- versions actively tested against. Subset of supported. */
	tested: ">=0.1.21",
	/** Known-broken versions. These are hard-blocked with a specific message. */
	blocked: [] as string[],
}

// ============================================================
// Types
// ============================================================

export interface DevoCheckResult {
	installed: boolean
	version: string | null
	path: string | null
	compatible: boolean
	compatibility: "ok" | "too-old" | "too-new" | "blocked" | "unknown"
	message: string | null
}

export type ExecFileForCheck = (
	cmd: string,
	args: string[],
	options: { env: Record<string, string | undefined>; timeout: number },
	callback: (err: Error | null, stdout: string) => void,
) => void

export interface CheckDevoProgramOptions {
	program: string
	env?: Record<string, string | undefined>
	execFile?: ExecFileForCheck
}

// ============================================================
// Binary detection
// ============================================================

/** Run a command and return stdout, or null on failure. */
function execAsync(
	cmd: string,
	args: string[],
	env: Record<string, string | undefined>,
	execFileImpl: ExecFileForCheck = execFile as unknown as ExecFileForCheck,
): Promise<string | null> {
	return new Promise((resolve) => {
		execFileImpl(cmd, args, { env, timeout: 5000 }, (err, stdout) => {
			if (err) {
				resolve(null)
				return
			}
			resolve(stdout.trim())
		})
	})
}

export async function checkDevoProgram({
	program,
	env = process.env,
	execFile: execFileImpl,
}: CheckDevoProgramOptions): Promise<DevoCheckResult> {
	log.info("Checking Devo runtime...", { program })
	const versionOutput = await execAsync(program, ["--version"], env, execFileImpl)
	if (!versionOutput) {
		return {
			installed: false,
			version: null,
			path: program,
			compatible: false,
			compatibility: "unknown",
			message: `Devo runtime not found at ${program}`,
		}
	}

	const version = parseDevoVersion(versionOutput)
	log.info("Devo runtime found", { version, path: program })
	return compatibilityResult(version, program)
}

function parseDevoVersion(versionOutput: string): string {
	const match = versionOutput.match(/v?(\d+\.\d+\.\d+(?:-[a-zA-Z0-9.]+)?)/)
	return match ? match[1] : versionOutput.trim()
}

function compatibilityResult(version: string, binaryPath: string | null): DevoCheckResult {
	// Coerce loose version strings (e.g. "1.3" -> "1.3.0") into valid semver.
	// Non-semver versions (e.g. "local", "dev", "unknown") are assumed compatible --
	// these are typically local/dev builds where the user knows what they're doing.
	const parsed = valid(version) ?? coerce(version)?.version ?? null
	if (!parsed) {
		log.info("Non-semver version detected, assuming compatible", { version })
		return {
			installed: true,
			version,
			path: binaryPath,
			compatible: true,
			compatibility: "ok",
			message: null,
		}
	}

	// Check blocked versions
	for (const blocked of DEVO_COMPAT.blocked) {
		if (satisfies(parsed, blocked)) {
			return {
				installed: true,
				version,
				path: binaryPath,
				compatible: false,
				compatibility: "blocked",
				message: `Devo ${version} has known issues with this version of Devo. Please update.`,
			}
		}
	}

	// Check supported range -- hard block if below minimum
	if (!satisfies(parsed, DEVO_COMPAT.supported)) {
		return {
			installed: true,
			version,
			path: binaryPath,
			compatible: false,
			compatibility: "too-old",
			message: `Devo ${version} is too old. Devo requires ${DEVO_COMPAT.supported}.`,
		}
	}

	// Check tested range -- supported but newer than what we've tested against
	if (!satisfies(parsed, DEVO_COMPAT.tested)) {
		return {
			installed: true,
			version,
			path: binaryPath,
			compatible: true,
			compatibility: "too-new",
			message: `Devo ${version} is newer than tested. Devo is tested with ${DEVO_COMPAT.tested}. Some features may not work as expected.`,
		}
	}

	// Within the tested range -- fully compatible
	return {
		installed: true,
		version,
		path: binaryPath,
		compatible: true,
		compatibility: "ok",
		message: null,
	}
}
