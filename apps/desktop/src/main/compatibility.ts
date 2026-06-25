/**
 * Devo CLI version compatibility definitions for Devo.
 *
 * Updated with each Devo release to reflect tested Devo versions.
 * The environment check in the onboarding flow uses these ranges to
 * decide whether to pass, warn, or block.
 */

import { execFile } from "node:child_process"
import { homedir } from "node:os"
import path from "node:path"
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

// ============================================================
// Binary detection
// ============================================================

/** Build the augmented PATH that includes ~/.devo/bin. */
function getAugmentedPath(): string {
	const devoBinDir = path.join(homedir(), ".devo", "bin")
	const sep = process.platform === "win32" ? ";" : ":"
	return `${devoBinDir}${sep}${process.env.PATH ?? ""}`
}

/** Run a command and return stdout, or null on failure. */
function execAsync(
	cmd: string,
	args: string[],
	env: Record<string, string | undefined>,
): Promise<string | null> {
	return new Promise((resolve) => {
		execFile(cmd, args, { env, timeout: 5000 }, (err, stdout) => {
			if (err) {
				resolve(null)
				return
			}
			resolve(stdout.trim())
		})
	})
}

/** Try to find the devo binary and get its version. */
async function detectDevo(): Promise<{ version: string | null; path: string | null }> {
	const augmentedPath = getAugmentedPath()
	const env = { ...process.env, PATH: augmentedPath }

	// Try `devo --version` (the correct flag)
	const versionOutput = await execAsync("devo", ["--version"], env)
	if (versionOutput) {
		// Parse version from output -- could be "v0.2.14", "devo v0.2.14", or "local"
		const match = versionOutput.match(/v?(\d+\.\d+\.\d+(?:-[a-zA-Z0-9.]+)?)/)
		const version = match ? match[1] : versionOutput.trim()

		// Try to find the path with `which` or `where`
		const whichCmd = process.platform === "win32" ? "where" : "which"
		const binaryPath = await execAsync(whichCmd, ["devo"], env)

		return { version, path: binaryPath }
	}

	// Fallback: check if the binary exists at all (might not support --version)
	const whichCmd = process.platform === "win32" ? "where" : "which"
	const binaryPath = await execAsync(whichCmd, ["devo"], env)
	if (binaryPath) {
		return { version: "unknown", path: binaryPath }
	}

	return { version: null, path: null }
}

// ============================================================
// Public API
// ============================================================

/**
 * Check whether Devo is installed and compatible with this version of Devo.
 * Runs the binary to get its version, then compares against the compatibility range.
 */
export async function checkDevo(): Promise<DevoCheckResult> {
	log.info("Checking Devo installation...")

	const { version, path: binaryPath } = await detectDevo()

	if (!version) {
		log.warn("Devo CLI not found")
		return {
			installed: false,
			version: null,
			path: null,
			compatible: false,
			compatibility: "unknown",
			message: "Devo CLI not found. Install it from https://devo.ai",
		}
	}

	log.info("Devo found", { version, path: binaryPath })

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
