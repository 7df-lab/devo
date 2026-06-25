/**
 * Writer module.
 *
 * Writes conversion results to the filesystem.
 * Supports dry-run mode, backup (via snapshot), and configurable merge strategies.
 */
import { createBackup } from "../backup"
import type { ConversionResult } from "../types/conversion-result"
import type { DevoConfig } from "../types/devo"
import { exists, safeReadFile, writeFileSafe } from "../utils/fs"
import { stringifyJson } from "../utils/json"
import * as paths from "../utils/paths"
import { type MergeStrategy, mergeConfigs } from "./merge"

export interface WriteOptions {
	/** Simulate writes without touching disk */
	dryRun?: boolean
	/** Back up all target files before writing (creates a restorable snapshot) */
	backup?: boolean
	/** Overwrite existing files */
	force?: boolean
	/** How to handle existing devo.json configs */
	mergeStrategy?: MergeStrategy
}

export interface WriteResult {
	/** Files that were written (or would be in dry-run) */
	filesWritten: string[]
	/** Files that were skipped because they already exist */
	filesSkipped: string[]
	/** Path to the backup snapshot directory (if backup was created) */
	backupDir?: string
	/**
	 * Individual backup file paths (legacy, for backwards compat with CLI output).
	 * @deprecated Use backupDir instead.
	 */
	backupPaths: string[]
}

/**
 * Collect all target file paths that the writer will touch.
 */
function collectTargetPaths(conversion: ConversionResult): string[] {
	const targetPaths: string[] = []

	if (Object.keys(conversion.globalConfig).length > 0) {
		targetPaths.push(paths.ocGlobalConfigPath())
	}

	for (const [projectPath] of conversion.projectConfigs) {
		targetPaths.push(paths.ocProjectConfigPath(projectPath))
	}

	for (const [targetPath] of conversion.agents) {
		targetPaths.push(targetPath)
	}
	for (const [targetPath] of conversion.commands) {
		targetPaths.push(targetPath)
	}
	for (const [targetPath] of conversion.rules) {
		targetPaths.push(targetPath)
	}
	for (const [targetPath] of conversion.hookPlugins) {
		targetPaths.push(targetPath)
	}

	if (conversion.sessions) {
		// SQLite mode (v1.2.0+): the database file is the only target
		targetPaths.push(paths.ocDatabasePath())
	}

	if (conversion.promptHistory && conversion.promptHistory.length > 0) {
		targetPaths.push(paths.ocPromptHistoryPath())
	}

	return targetPaths
}

/**
 * Write conversion results to disk.
 *
 * @param conversion - Output from `convert()`
 * @param options - Write options (dry-run, backup, force, merge strategy)
 * @returns Summary of files written/skipped/backed up
 */
export async function write(
	conversion: ConversionResult,
	options: WriteOptions = {},
): Promise<WriteResult> {
	const {
		dryRun = false,
		backup = false,
		force = false,
		mergeStrategy = "preserve-existing",
	} = options

	const result: WriteResult = {
		filesWritten: [],
		filesSkipped: [],
		backupPaths: [],
	}

	// ─── Create backup snapshot before any writes ─────────────────────
	if (backup && !dryRun) {
		const targetPaths = collectTargetPaths(conversion)
		const backupDir = await createBackup(targetPaths, "Pre-migration backup")
		if (backupDir) {
			result.backupDir = backupDir
		}
	}

	// ─── Write global config ─────────────────────────────────────────
	if (Object.keys(conversion.globalConfig).length > 0) {
		const globalConfigPath = paths.ocGlobalConfigPath()
		await writeConfigFile(
			globalConfigPath,
			conversion.globalConfig,
			{ dryRun, force, mergeStrategy },
			result,
		)
	}

	// ─── Write per-project configs ───────────────────────────────────
	for (const [projectPath, config] of conversion.projectConfigs) {
		const configPath = paths.ocProjectConfigPath(projectPath)
		await writeConfigFile(configPath, config, { dryRun, force, mergeStrategy }, result)
	}

	// ─── Write agent files ───────────────────────────────────────────
	for (const [targetPath, content] of conversion.agents) {
		await writeFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write command files ─────────────────────────────────────────
	for (const [targetPath, content] of conversion.commands) {
		await writeFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write rules files (AGENTS.md) ───────────────────────────────
	for (const [targetPath, content] of conversion.rules) {
		await writeFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write hook plugin stubs ─────────────────────────────────────
	for (const [targetPath, content] of conversion.hookPlugins) {
		await writeFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write session history to SQLite (if present) ───────────────
	if (conversion.sessions && conversion.sessions.length > 0 && !dryRun) {
		const { writeHistorySessionsDetailed } = await import("./history")
		const historyResult = await writeHistorySessionsDetailed(conversion.sessions)
		result.filesWritten.push(...historyResult.filesWritten)
	} else if (conversion.sessions && conversion.sessions.length > 0 && dryRun) {
		// In dry-run mode, report what would be written
		for (const session of conversion.sessions) {
			result.filesWritten.push(`sqlite:session:${session.session.id}`)
			for (const message of session.messages) {
				result.filesWritten.push(`sqlite:message:${message.id}`)
			}
		}
	}

	// ─── Write prompt history (if present) ───────────────────────────
	if (conversion.promptHistory && conversion.promptHistory.length > 0) {
		const historyPath = paths.ocPromptHistoryPath()
		const lines = `${conversion.promptHistory.map((e) => JSON.stringify(e)).join("\n")}\n`

		if (!dryRun) {
			const existingContent = await safeReadFile(historyPath)
			const finalContent = existingContent ? existingContent + lines : lines
			await writeFileSafe(historyPath, finalContent)
		}
		result.filesWritten.push(historyPath)
	}

	return result
}

// ─── Internal helpers ────────────────────────────────────────────────

async function writeConfigFile(
	filePath: string,
	config: Partial<DevoConfig>,
	options: {
		dryRun: boolean
		force: boolean
		mergeStrategy: MergeStrategy
	},
	result: WriteResult,
): Promise<void> {
	const existingContent = await safeReadFile(filePath)

	if (existingContent) {
		if (!options.force && options.mergeStrategy === "overwrite") {
			result.filesSkipped.push(filePath)
			return
		}

		let existingConfig: Partial<DevoConfig> = {}
		try {
			existingConfig = JSON.parse(existingContent) as Partial<DevoConfig>
		} catch {
			// Existing file is malformed -- treat as empty
		}

		const merged = mergeConfigs(existingConfig, config, options.mergeStrategy)

		if (!options.dryRun) {
			await writeFileSafe(filePath, stringifyJson(merged))
		}
		result.filesWritten.push(filePath)
	} else {
		if (!options.dryRun) {
			await writeFileSafe(filePath, stringifyJson(config))
		}
		result.filesWritten.push(filePath)
	}
}

async function writeFile(
	filePath: string,
	content: string,
	options: { dryRun: boolean; force: boolean },
	result: WriteResult,
): Promise<void> {
	const fileExists = await exists(filePath)

	if (fileExists && !options.force) {
		result.filesSkipped.push(filePath)
		return
	}

	if (!options.dryRun) {
		await writeFileSafe(filePath, content)
	}
	result.filesWritten.push(filePath)
}

export { type MergeStrategy, mergeConfigs } from "./merge"
