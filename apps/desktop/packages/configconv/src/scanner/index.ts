/**
 * Main scanner entry point.
 *
 * Discovers configuration files for all supported agent formats:
 * - Claude Code: ~/.Claude/, ~/.claude.json, .claude/, .mcp.json, CLAUDE.md
 * - Devo: ~/.config/devo/, .devo/, devo.json, AGENTS.md
 * - Cursor: ~/.cursor/, .cursor/, .cursorrules
 */

import type { DevoScanResult } from "../converter/to-canonical/devo"
import type { AgentFormat } from "../types/canonical"
import type { CursorScanResult } from "../types/cursor"
import type { ScanOptions, ScanResult } from "../types/scan-result"
import { scanGlobal, scanHistory, scanProject } from "./claude-config"
import { scanCursorGlobal, scanCursorProject } from "./cursor-config"
import { scanCursorHistory } from "./cursor-history"
import { scanDevoGlobal, scanDevoProject } from "./devo-config"

// ============================================================
// Claude Code scanner (preserved for backwards compatibility)
// ============================================================

/**
 * Scan for Claude Code configuration files.
 *
 * @param options - What to scan (global, specific project, history)
 * @returns Structured scan result with all discovered config data
 */
export async function scan(options: ScanOptions = {}): Promise<ScanResult> {
	const { global: scanGlobalConfig = true, project, includeHistory = false, since } = options

	const result: ScanResult = {
		global: { skills: [] },
		projects: [],
	}

	// Scan global config
	if (scanGlobalConfig) {
		result.global = await scanGlobal()
	}

	// Scan project(s)
	if (project) {
		const projectResult = await scanProject(project, result.global.userState)
		result.projects.push(projectResult)
	} else if (result.global.userState?.projects) {
		// Scan all known projects from ~/.claude.json
		const projectPaths = Object.keys(result.global.userState.projects)
		for (const projectPath of projectPaths) {
			const projectResult = await scanProject(projectPath, result.global.userState)
			result.projects.push(projectResult)
		}
	}

	// Scan history
	if (includeHistory) {
		result.history = await scanHistory(since)
	}

	return result
}

// ============================================================
// Cursor scanner
// ============================================================

export interface CursorScanOptions {
	/** Scan global Cursor config (~/.cursor/) */
	global?: boolean
	/** Scan specific project path */
	project?: string
	/** Include chat history from state.vscdb databases */
	includeHistory?: boolean
	/** Only import history since this date */
	since?: Date
}

/**
 * Scan for Cursor IDE configuration files.
 */
export async function scanCursor(options: CursorScanOptions = {}): Promise<CursorScanResult> {
	const { global: scanGlobalConfig = true, project, includeHistory = false, since } = options

	const result: CursorScanResult = {
		global: { skills: [], commands: [], agents: [] },
		projects: [],
	}

	if (scanGlobalConfig) {
		result.global = await scanCursorGlobal()
	}

	if (project) {
		const projectResult = await scanCursorProject(project)
		result.projects.push(projectResult)
	}

	if (includeHistory) {
		result.history = await scanCursorHistory(since)
	}

	return result
}

// ============================================================
// Devo scanner
// ============================================================

export interface DevoScanOptions {
	/** Scan global Devo config (~/.config/devo/) */
	global?: boolean
	/** Scan specific project path */
	project?: string
}

/**
 * Scan for Devo configuration files.
 */
export async function scanDevo(options: DevoScanOptions = {}): Promise<DevoScanResult> {
	const { global: scanGlobalConfig = true, project } = options

	const result: DevoScanResult = {
		global: { agents: [], commands: [], skills: [] },
		projects: [],
	}

	if (scanGlobalConfig) {
		result.global = await scanDevoGlobal()
	}

	if (project) {
		const projectResult = await scanDevoProject(project)
		result.projects.push(projectResult)
	}

	return result
}

// ============================================================
// Universal scanner
// ============================================================

export interface UniversalScanOptions {
	/** Which format to scan */
	format: AgentFormat
	/** Scan global config */
	global?: boolean
	/** Scan specific project path */
	project?: string
	/** Include session history (Claude Code, Cursor) */
	includeHistory?: boolean
	/** Only import history since this date (Claude Code, Cursor) */
	since?: Date
}

/**
 * Scan for configuration files of a specific format.
 */
export async function scanFormat(
	options: UniversalScanOptions,
): Promise<
	| { format: "claude-code"; data: ScanResult }
	| { format: "devo"; data: DevoScanResult }
	| { format: "cursor"; data: CursorScanResult }
> {
	switch (options.format) {
		case "claude-code": {
			const data = await scan({
				global: options.global,
				project: options.project,
				includeHistory: options.includeHistory,
				since: options.since,
			})
			return { format: "claude-code", data }
		}
		case "devo": {
			const data = await scanDevo({
				global: options.global,
				project: options.project,
			})
			return { format: "devo", data }
		}
		case "cursor": {
			const data = await scanCursor({
				global: options.global,
				project: options.project,
				includeHistory: options.includeHistory,
				since: options.since,
			})
			return { format: "cursor", data }
		}
	}
}
