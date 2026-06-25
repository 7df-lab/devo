/**
 * Converter orchestrator.
 *
 * Ties together all individual converters into a single `convert()` pipeline.
 * This function is mostly pure -- the only I/O is realpath() for skill dedup on macOS.
 */
import type {
	ConversionResult,
	ConvertOptions,
	MigrationCategory,
} from "../types/conversion-result"
import type { DevoConfig } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport, mergeReports } from "../types/report"
import type { ScanResult } from "../types/scan-result"
import { resolveRealPath } from "../utils/fs"
import { convertAgents } from "./agents"
import { convertCommands } from "./commands"
import { convertConfig } from "./config"
import { convertHistory } from "./history"
import { convertHooks } from "./hooks"
import { convertMcpServers, mergeMcpSources } from "./mcp"
import { convertPermissions } from "./permissions"
import { convertRules } from "./rules"
import { verifySkills } from "./skills"

const ALL_CATEGORIES: MigrationCategory[] = [
	"config",
	"mcp",
	"agents",
	"commands",
	"skills",
	"permissions",
	"rules",
	"hooks",
]

/**
 * Convert Claude Code scan results to Devo format.
 *
 * @param scanResult - Output from `scan()`
 * @param options - Conversion options (category filter, model overrides, etc.)
 * @returns Conversion result with config, files, and migration report
 */
export async function convert(
	scanResult: ScanResult,
	options: ConvertOptions = {},
): Promise<ConversionResult> {
	const categories = new Set(options.categories ?? ALL_CATEGORIES)
	const includeHistory = options.includeHistory ?? false

	const result: ConversionResult = {
		globalConfig: {},
		projectConfigs: new Map(),
		agents: new Map(),
		commands: new Map(),
		rules: new Map(),
		hookPlugins: new Map(),
		report: createEmptyReport(),
	}

	const reports: MigrationReport[] = []

	// ─── Global config conversion ────────────────────────────────────
	if (categories.has("config")) {
		const { config, report } = convertConfig({
			settings: scanResult.global.settings,
			userState: scanResult.global.userState,
			modelOverrides: options.modelOverrides,
			defaultModel: options.defaultModel,
			defaultSmallModel: options.defaultSmallModel,
		})
		Object.assign(result.globalConfig, config)
		reports.push(report)
	}

	// ─── Global MCP servers (from settings) ──────────────────────────
	if (categories.has("mcp") && scanResult.global.settings?.env) {
		// Global MCP servers can come from ~/.claude.json project entries
		// but global-level MCP is typically per-project. We handle it below.
	}

	// ─── Global skills ───────────────────────────────────────────────
	// Track globally-reported skill paths to avoid duplicates per-project.
	// On macOS, ~/.Claude/skills/ and ~/.claude/skills/ resolve to the same
	// directory (case-insensitive FS), so we normalize via realpath.
	const reportedSkillPaths = new Set<string>()

	if (categories.has("skills") && scanResult.global.skills.length > 0) {
		const { report } = verifySkills(scanResult.global.skills)
		reports.push(report)
		for (const skill of scanResult.global.skills) {
			reportedSkillPaths.add(await resolveRealPath(skill.path))
		}
	}

	// ─── Global rules (CLAUDE.md -> AGENTS.md) ───────────────────────
	if (categories.has("rules") && scanResult.global.claudeMd) {
		const { agentsMd, report } = convertRules({
			claudeMd: scanResult.global.claudeMd,
			claudeMdPath: scanResult.global.claudeMdPath,
			projectPath: "~",
		})
		if (agentsMd) {
			result.rules.set("~/.config/devo/AGENTS.md", agentsMd)
		}
		reports.push(report)
	}

	// ─── Global hooks ────────────────────────────────────────────────
	if (categories.has("hooks") && scanResult.global.settings?.hooks) {
		const { plugins, report } = convertHooks(scanResult.global.settings.hooks)
		for (const [path, content] of plugins) {
			result.hookPlugins.set(`~/.config/devo/plugins/${path}`, content)
		}
		reports.push(report)
	}

	// ─── Per-project conversion ──────────────────────────────────────
	for (const project of scanResult.projects) {
		const projectConfig: Partial<DevoConfig> = {}

		// Project MCP servers (merged from .mcp.json + ~/.claude.json per-project)
		if (categories.has("mcp")) {
			const mcpSources = []

			// From .mcp.json
			if (project.mcpJson?.mcpServers) {
				mcpSources.push({
					servers: project.mcpJson.mcpServers,
					disabledServers: project.disabledMcpServers,
					sourceDescription: `.mcp.json (${project.path})`,
				})
			}

			// From ~/.claude.json per-project mcpServers
			if (Object.keys(project.projectMcpServers).length > 0) {
				mcpSources.push({
					servers: project.projectMcpServers,
					sourceDescription: `~/.claude.json projects[${project.path}]`,
				})
			}

			// From .claude/settings.local.json
			if (project.settingsLocal?.mcpServers) {
				mcpSources.push({
					servers: project.settingsLocal.mcpServers,
					sourceDescription: `.claude/settings.local.json (${project.path})`,
				})
			}

			if (mcpSources.length > 0) {
				const merged = mergeMcpSources(...mcpSources)
				const { mcp, report } = convertMcpServers(merged)
				if (Object.keys(mcp).length > 0) {
					projectConfig.mcp = mcp
				}
				reports.push(report)
			}
		}

		// Project permissions
		if (categories.has("permissions")) {
			const { permission, report } = convertPermissions(
				project.settingsLocal?.permissions,
				project.allowedTools,
			)
			if (Object.keys(permission).length > 0) {
				projectConfig.permission = permission
			}
			reports.push(report)
		}

		// Project model override
		if (project.settingsLocal?.model) {
			const { config, report } = convertConfig({
				settings: {
					model: project.settingsLocal.model,
					env: project.settingsLocal.env,
				},
				modelOverrides: options.modelOverrides,
			})
			if (config.model) {
				projectConfig.model = config.model
			}
			if (config.small_model) {
				projectConfig.small_model = config.small_model
			}
			reports.push(report)
		}

		// Project agents
		if (categories.has("agents") && project.agents.length > 0) {
			const { agents, report } = convertAgents({
				agents: project.agents,
				modelOverrides: options.modelOverrides,
			})
			for (const [filename, content] of agents) {
				result.agents.set(`${project.path}/.devo/agents/${filename}`, content)
			}
			reports.push(report)
		}

		// Project commands
		if (categories.has("commands") && project.commands.length > 0) {
			const { commands, report } = convertCommands(project.commands)
			for (const [filename, content] of commands) {
				result.commands.set(`${project.path}/.devo/commands/${filename}`, content)
			}
			reports.push(report)
		}

		// Project skills (deduplicate against already-reported global skills)
		if (categories.has("skills") && project.skills.length > 0) {
			const newSkills: typeof project.skills = []
			for (const skill of project.skills) {
				const realPath = await resolveRealPath(skill.path)
				if (!reportedSkillPaths.has(realPath)) {
					newSkills.push(skill)
					reportedSkillPaths.add(realPath)
				}
			}
			if (newSkills.length > 0) {
				const { report } = verifySkills(newSkills)
				reports.push(report)
			}
		}

		// Project rules (CLAUDE.md -> AGENTS.md)
		if (categories.has("rules")) {
			const { agentsMd, report } = convertRules({
				claudeMd: project.claudeMd,
				claudeMdPath: project.claudeMdPath,
				agentsMd: project.agentsMd,
				agentsMdPath: project.agentsMdPath,
				projectPath: project.path,
			})
			if (agentsMd) {
				result.rules.set(`${project.path}/AGENTS.md`, agentsMd)
			}
			reports.push(report)
		}

		// Store project config if non-empty
		const configKeys = Object.keys(projectConfig)
		if (configKeys.length > 0) {
			result.projectConfigs.set(project.path, projectConfig)
		}
	}

	// ─── History (opt-in) ────────────────────────────────────────────
	if (includeHistory && scanResult.history) {
		const { sessions, promptHistory, report } = await convertHistory(scanResult.history)
		result.sessions = sessions
		result.promptHistory = promptHistory
		reports.push(report)
	}

	// ─── Merge all reports ───────────────────────────────────────────
	result.report = mergeReports(...reports)

	return result
}
