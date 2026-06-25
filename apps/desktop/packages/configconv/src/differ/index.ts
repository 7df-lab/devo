/**
 * Differ module.
 *
 * Compares Claude Code scan results against current Devo configuration
 * to show what's different and what a migration would change.
 */
import type { DevoConfig } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import type { ScanResult } from "../types/scan-result"
import { safeReadFile, safeReadJson } from "../utils/fs"
import * as paths from "../utils/paths"

export interface DiffResult {
	/** Items that exist in CC but not in OC */
	onlyInClaudeCode: DiffItem[]
	/** Items that exist in OC but not in CC */
	onlyInDevo: DiffItem[]
	/** Items that exist in both but differ */
	different: DiffItem[]
	/** Items that match between CC and OC */
	matching: DiffItem[]
	report: MigrationReport
}

export interface DiffItem {
	category: string
	key: string
	claudeCodeValue?: unknown
	openCodeValue?: unknown
	details?: string
}

/**
 * Compare Claude Code scan results against current Devo configuration.
 *
 * @param scanResult - Output from `scan()`
 * @returns Diff showing what's different between CC and OC configs
 */
export async function diff(scanResult: ScanResult): Promise<DiffResult> {
	const result: DiffResult = {
		onlyInClaudeCode: [],
		onlyInDevo: [],
		different: [],
		matching: [],
		report: createEmptyReport(),
	}

	// Load current Devo global config
	const ocConfigPath = paths.ocGlobalConfigPath()
	const ocConfig = await safeReadJson<Partial<DevoConfig>>(ocConfigPath)

	// ─── Compare model ───────────────────────────────────────────────
	if (scanResult.global.settings?.model) {
		if (ocConfig?.model) {
			const ccModel = scanResult.global.settings.model
			const ocModel = ocConfig.model
			if (ccModel === ocModel || ocModel.endsWith(`/${ccModel}`)) {
				result.matching.push({
					category: "config",
					key: "model",
					claudeCodeValue: ccModel,
					openCodeValue: ocModel,
				})
			} else {
				result.different.push({
					category: "config",
					key: "model",
					claudeCodeValue: ccModel,
					openCodeValue: ocModel,
					details: "Model IDs differ (may be equivalent with different naming)",
				})
			}
		} else {
			result.onlyInClaudeCode.push({
				category: "config",
				key: "model",
				claudeCodeValue: scanResult.global.settings.model,
			})
		}
	}

	// ─── Compare MCP servers ─────────────────────────────────────────
	for (const project of scanResult.projects) {
		// Load project-level OC config
		const projectOcConfig = await safeReadJson<Partial<DevoConfig>>(
			paths.ocProjectConfigPath(project.path),
		)

		const ccMcpNames = new Set<string>()

		// Collect all CC MCP server names
		if (project.mcpJson?.mcpServers) {
			for (const name of Object.keys(project.mcpJson.mcpServers)) {
				ccMcpNames.add(name)
			}
		}
		for (const name of Object.keys(project.projectMcpServers)) {
			ccMcpNames.add(name)
		}

		const ocMcpNames = new Set(Object.keys(projectOcConfig?.mcp ?? {}))

		// In CC but not OC
		for (const name of ccMcpNames) {
			if (!ocMcpNames.has(name)) {
				result.onlyInClaudeCode.push({
					category: "mcp",
					key: name,
					claudeCodeValue: project.mcpJson?.mcpServers?.[name] ?? project.projectMcpServers[name],
					details: `Project: ${project.path}`,
				})
			} else {
				result.matching.push({
					category: "mcp",
					key: name,
					details: `Present in both CC and OC for ${project.path}`,
				})
			}
		}

		// In OC but not CC
		for (const name of ocMcpNames) {
			if (!ccMcpNames.has(name)) {
				result.onlyInDevo.push({
					category: "mcp",
					key: name,
					openCodeValue: projectOcConfig?.mcp?.[name],
					details: `Project: ${project.path}`,
				})
			}
		}

		// ─── Compare agents ──────────────────────────────────────────
		const ocAgentsDir = paths.ocProjectAgentsDir(project.path)
		for (const agent of project.agents) {
			const ocAgentPath = `${ocAgentsDir}/${agent.name}.md`
			const ocAgentContent = await safeReadFile(ocAgentPath)
			if (ocAgentContent) {
				result.matching.push({
					category: "agents",
					key: agent.name,
					details: `Present in both CC and OC for ${project.path}`,
				})
			} else {
				result.onlyInClaudeCode.push({
					category: "agents",
					key: agent.name,
					claudeCodeValue: agent.path,
					details: `Project: ${project.path}`,
				})
			}
		}

		// ─── Compare commands ────────────────────────────────────────
		const ocCommandsDir = paths.ocProjectCommandsDir(project.path)
		for (const cmd of project.commands) {
			const ocCmdPath = `${ocCommandsDir}/${cmd.name}.md`
			const ocCmdContent = await safeReadFile(ocCmdPath)
			if (ocCmdContent) {
				result.matching.push({
					category: "commands",
					key: cmd.name,
					details: `Present in both CC and OC for ${project.path}`,
				})
			} else {
				result.onlyInClaudeCode.push({
					category: "commands",
					key: cmd.name,
					claudeCodeValue: cmd.path,
					details: `Project: ${project.path}`,
				})
			}
		}

		// ─── Compare rules (CLAUDE.md vs AGENTS.md) ──────────────────
		if (project.claudeMd && !project.agentsMd) {
			result.onlyInClaudeCode.push({
				category: "rules",
				key: "CLAUDE.md",
				details: `${project.path}: CLAUDE.md exists but no AGENTS.md`,
			})
		} else if (project.claudeMd && project.agentsMd) {
			result.matching.push({
				category: "rules",
				key: "rules",
				details: `${project.path}: Both CLAUDE.md and AGENTS.md exist`,
			})
		}
	}

	// ─── Compare permissions ─────────────────────────────────────────
	if (scanResult.global.settings?.permissions) {
		if (ocConfig?.permission) {
			result.different.push({
				category: "permissions",
				key: "global",
				claudeCodeValue: scanResult.global.settings.permissions,
				openCodeValue: ocConfig.permission,
				details: "Permission formats differ -- manual comparison needed",
			})
		} else {
			result.onlyInClaudeCode.push({
				category: "permissions",
				key: "global",
				claudeCodeValue: scanResult.global.settings.permissions,
			})
		}
	}

	// ─── Summary report ──────────────────────────────────────────────
	const report = result.report
	if (result.onlyInClaudeCode.length > 0) {
		report.manualActions.push(
			`${result.onlyInClaudeCode.length} item(s) exist in Claude Code but not Devo. Run \`configconv migrate\` to convert them.`,
		)
	}
	if (result.different.length > 0) {
		report.warnings.push(
			`${result.different.length} item(s) differ between Claude Code and Devo configs.`,
		)
	}
	if (result.matching.length > 0) {
		report.migrated.push({
			category: "config",
			source: "diff",
			target: "diff",
			details: `${result.matching.length} item(s) match between CC and OC`,
		})
	}

	return result
}
