/**
 * Universal writer module.
 *
 * Writes CanonicalConversionResult to the filesystem for any target format.
 * Supports dry-run mode, backup (via snapshot), and configurable merge strategies.
 */
import { createBackup } from "../backup"
import type { AgentFormat, CanonicalConversionResult } from "../types/canonical"
import { exists, safeReadFile, writeFileSafe } from "../utils/fs"
import { stringifyJson } from "../utils/json"
import * as paths from "../utils/paths"

export interface UniversalWriteOptions {
	/** Simulate writes without touching disk */
	dryRun?: boolean
	/** Back up all target files before writing (creates a restorable snapshot) */
	backup?: boolean
	/** Overwrite existing files */
	force?: boolean
	/** How to handle existing config files */
	mergeStrategy?: "preserve-existing" | "overwrite" | "merge"
}

export interface UniversalWriteResult {
	/** Source format that was converted from */
	sourceFormat: AgentFormat
	/** Target format that was written */
	targetFormat: AgentFormat
	/** Files that were written (or would be in dry-run) */
	filesWritten: string[]
	/** Files that were skipped because they already exist */
	filesSkipped: string[]
	/** Path to the backup snapshot directory (if backup was created) */
	backupDir?: string
}

/**
 * Get the config file path for a target format.
 */
function getGlobalConfigPath(format: AgentFormat): string {
	switch (format) {
		case "devo":
			return paths.ocGlobalConfigPath()
		case "claude-code":
			return paths.ccSettingsPath()
		case "cursor":
			return paths.cursorGlobalMcpJsonPath()
	}
}

/**
 * Get the project config file path for a target format.
 */
function getProjectConfigPath(format: AgentFormat, projectPath: string): string {
	switch (format) {
		case "devo":
			return paths.ocProjectConfigPath(projectPath)
		case "claude-code":
			return paths.ccProjectMcpJsonPath(projectPath)
		case "cursor":
			return paths.cursorProjectMcpJsonPath(projectPath)
	}
}

/**
 * Collect all target file paths that the writer will touch.
 */
function collectTargetPaths(conversion: CanonicalConversionResult): string[] {
	const targetPaths: string[] = []

	if (Object.keys(conversion.globalConfig).length > 0) {
		targetPaths.push(getGlobalConfigPath(conversion.targetFormat))
	}

	for (const [projectPath] of conversion.projectConfigs) {
		targetPaths.push(getProjectConfigPath(conversion.targetFormat, projectPath))
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
	for (const [targetPath] of conversion.extraFiles) {
		targetPaths.push(targetPath)
	}

	return targetPaths
}

/**
 * Write universal conversion results to disk.
 *
 * Works with any target format (Devo, Claude Code, Cursor).
 * The conversion result already contains absolute target paths for
 * agents, commands, rules, and extraFiles.
 *
 * @param conversion - Output from `universalConvert()`
 * @param options - Write options (dry-run, backup, force, merge strategy)
 * @returns Summary of files written/skipped/backed up
 */
export async function universalWrite(
	conversion: CanonicalConversionResult,
	options: UniversalWriteOptions = {},
): Promise<UniversalWriteResult> {
	const {
		dryRun = false,
		backup = false,
		force = false,
		mergeStrategy = "preserve-existing",
	} = options

	const result: UniversalWriteResult = {
		sourceFormat: conversion.sourceFormat,
		targetFormat: conversion.targetFormat,
		filesWritten: [],
		filesSkipped: [],
	}

	// ─── Create backup snapshot before any writes ─────────────────────
	if (backup && !dryRun) {
		const targetPaths = collectTargetPaths(conversion)
		const backupDir = await createBackup(targetPaths, "Pre-conversion backup")
		if (backupDir) {
			result.backupDir = backupDir
		}
	}

	// ─── Write global config ─────────────────────────────────────────
	if (Object.keys(conversion.globalConfig).length > 0) {
		const configPath = getGlobalConfigPath(conversion.targetFormat)
		await writeJsonFile(
			configPath,
			conversion.globalConfig,
			{ dryRun, force, mergeStrategy },
			result,
		)
	}

	// ─── Write per-project configs ───────────────────────────────────
	for (const [projectPath, config] of conversion.projectConfigs) {
		const configPath = getProjectConfigPath(conversion.targetFormat, projectPath)
		await writeJsonFile(configPath, config, { dryRun, force, mergeStrategy }, result)
	}

	// ─── Write agent files ───────────────────────────────────────────
	for (const [targetPath, content] of conversion.agents) {
		await writeTextFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write command files ─────────────────────────────────────────
	for (const [targetPath, content] of conversion.commands) {
		await writeTextFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write rules files ───────────────────────────────────────────
	for (const [targetPath, content] of conversion.rules) {
		await writeTextFile(targetPath, content, { dryRun, force }, result)
	}

	// ─── Write extra files (plugins, etc.) ───────────────────────────
	for (const [targetPath, content] of conversion.extraFiles) {
		await writeTextFile(targetPath, content, { dryRun, force }, result)
	}

	return result
}

// ─── Internal helpers ────────────────────────────────────────────────

async function writeJsonFile(
	filePath: string,
	config: Record<string, unknown>,
	options: {
		dryRun: boolean
		force: boolean
		mergeStrategy: "preserve-existing" | "overwrite" | "merge"
	},
	result: UniversalWriteResult,
): Promise<void> {
	const existingContent = await safeReadFile(filePath)

	if (existingContent) {
		if (!options.force && options.mergeStrategy === "overwrite") {
			result.filesSkipped.push(filePath)
			return
		}

		let existingConfig: Record<string, unknown> = {}
		try {
			existingConfig = JSON.parse(existingContent) as Record<string, unknown>
		} catch {
			// Existing file is malformed, treat as empty
		}

		const merged = mergeRecords(existingConfig, config, options.mergeStrategy)

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

async function writeTextFile(
	filePath: string,
	content: string,
	options: { dryRun: boolean; force: boolean },
	result: UniversalWriteResult,
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

/**
 * Generic record merge (works for any JSON config format).
 */
function mergeRecords(
	existing: Record<string, unknown>,
	incoming: Record<string, unknown>,
	strategy: "preserve-existing" | "overwrite" | "merge",
): Record<string, unknown> {
	if (strategy === "overwrite") {
		return { ...existing, ...incoming }
	}

	// "preserve-existing" and "merge" both preserve existing values
	const result = { ...existing }
	for (const [key, value] of Object.entries(incoming)) {
		const existingVal = result[key]

		if (existingVal === undefined) {
			result[key] = value
		} else if (strategy === "merge" && isPlainObject(existingVal) && isPlainObject(value)) {
			result[key] = mergeRecords(
				existingVal as Record<string, unknown>,
				value as Record<string, unknown>,
				strategy,
			)
		}
		// "preserve-existing": skip if key exists
		// "merge" with non-object: skip (keep existing)
	}

	return result
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value)
}
