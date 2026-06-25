/**
 * Canonical -> Devo format converter.
 *
 * Produces Devo-compatible configuration files from canonical representation.
 */
import type {
	CanonicalAgentFile,
	CanonicalCommandFile,
	CanonicalConversionResult,
	CanonicalMcpServer,
	CanonicalProjectConfig,
	CanonicalRulesFile,
	CanonicalScanResult,
	ConversionReport,
} from "../../types/canonical"
import { createEmptyReport, mergeReports } from "../../types/canonical"
import type { DevoCommandFrontmatter } from "../../types/devo"
import * as paths from "../../utils/paths"
import { serializeFrontmatter } from "../../utils/yaml"
import { detectProvider, suggestSmallModel, translateModelId } from "../model-id"

/**
 * Convert canonical scan result to Devo output files.
 */
export function canonicalToDevo(
	scan: CanonicalScanResult,
	options?: {
		modelOverrides?: Record<string, string>
		defaultModel?: string
		defaultSmallModel?: string
	},
): CanonicalConversionResult {
	const result: CanonicalConversionResult = {
		sourceFormat: scan.sourceFormat,
		targetFormat: "devo",
		globalConfig: {},
		projectConfigs: new Map(),
		agents: new Map(),
		commands: new Map(),
		rules: new Map(),
		extraFiles: new Map(),
		report: createEmptyReport(),
	}

	const reports: ConversionReport[] = []

	// ─── Global config ───────────────────────────────────────────────
	const { config: globalConfig, report: globalReport } = convertGlobalToOC(scan.global, options)
	result.globalConfig = globalConfig
	reports.push(globalReport)

	// ─── Global agents ───────────────────────────────────────────────
	for (const agent of scan.global.agents) {
		const { content, report } = convertAgentToOC(agent, options?.modelOverrides)
		result.agents.set(`${paths.ocGlobalAgentsDir()}/${agent.name}.md`, content)
		reports.push(report)
	}

	// ─── Global commands ─────────────────────────────────────────────
	for (const cmd of scan.global.commands) {
		const { content, report } = convertCommandToOC(cmd)
		result.commands.set(`${paths.ocGlobalCommandsDir()}/${cmd.name}.md`, content)
		reports.push(report)
	}

	// ─── Global rules -> AGENTS.md ───────────────────────────────────
	if (scan.global.rules && scan.global.rules.length > 0) {
		const combined = combineRulesForAgentsMd(scan.global.rules)
		if (combined) {
			result.rules.set(paths.ocGlobalAgentsMdPath(), combined)
		}
	}

	// ─── Per-project conversion ──────────────────────────────────────
	for (const project of scan.projects) {
		const { report: projectReport } = convertProjectToOC(project, result, options)
		reports.push(projectReport)
	}

	result.report = mergeReports(...reports)
	return result
}

function convertGlobalToOC(
	global: CanonicalScanResult["global"],
	options?: {
		modelOverrides?: Record<string, string>
		defaultModel?: string
		defaultSmallModel?: string
	},
): { config: Record<string, unknown>; report: ConversionReport } {
	const report = createEmptyReport()
	const config: Record<string, unknown> = {
		$schema: "https://devo.ai/config.json",
	}

	// Model
	if (global.model) {
		const provider = detectProvider(global.env, global.model)
		const model = translateModelId(global.model, provider, options?.modelOverrides)
		config.model = model
		config.small_model = options?.defaultSmallModel ?? suggestSmallModel(model)

		report.converted.push({
			category: "config",
			source: `model: "${global.model}"`,
			target: `model: "${model}"`,
		})
	} else if (options?.defaultModel) {
		config.model = options.defaultModel
	}

	// Provider from env vars
	if (global.env) {
		const providerConfig = buildProviderConfig(global.env, report)
		if (Object.keys(providerConfig).length > 0) {
			config.provider = providerConfig
		}
	}

	// MCP servers
	if (Object.keys(global.mcpServers).length > 0) {
		const mcp: Record<string, unknown> = {}
		for (const [name, server] of Object.entries(global.mcpServers)) {
			mcp[name] = convertMcpToOC(server)
			report.converted.push({
				category: "mcp",
				source: `global: ${name}`,
				target: `mcp.${name}`,
				details: `${server.type} server`,
			})
		}
		config.mcp = mcp
	}

	// Permissions
	if (global.permissions) {
		config.permission = global.permissions
		report.converted.push({
			category: "permissions",
			source: "global permissions",
			target: "permission",
		})
	}

	// Auto-update
	if (global.autoUpdate !== undefined) {
		config.autoupdate = global.autoUpdate
	}

	return { config, report }
}

function convertProjectToOC(
	project: CanonicalProjectConfig,
	result: CanonicalConversionResult,
	options?: { modelOverrides?: Record<string, string> },
): { report: ConversionReport } {
	const report = createEmptyReport()
	const projectConfig: Record<string, unknown> = {}

	// Model
	if (project.model) {
		const provider = detectProvider(undefined, project.model)
		projectConfig.model = translateModelId(project.model, provider, options?.modelOverrides)
	}

	// MCP servers
	if (Object.keys(project.mcpServers).length > 0) {
		const mcp: Record<string, unknown> = {}
		for (const [name, server] of Object.entries(project.mcpServers)) {
			mcp[name] = convertMcpToOC(server)
			report.converted.push({
				category: "mcp",
				source: `${project.path}: ${name}`,
				target: `mcp.${name}`,
				details: `${server.type} server`,
			})
		}
		projectConfig.mcp = mcp
	}

	// Permissions
	if (project.permissions) {
		projectConfig.permission = project.permissions
	}

	// Store project config
	if (Object.keys(projectConfig).length > 0) {
		result.projectConfigs.set(project.path, projectConfig)
	}

	// Rules -> AGENTS.md
	const alwaysRules = project.rules.filter(
		(r) => r.ruleType === "always" || r.alwaysApply || r.ruleType === "general",
	)
	if (alwaysRules.length > 0) {
		const combined = combineRulesForAgentsMd(alwaysRules)
		if (combined) {
			result.rules.set(`${project.path}/AGENTS.md`, combined)
			report.converted.push({
				category: "rules",
				source: `${alwaysRules.length} rules`,
				target: `${project.path}/AGENTS.md`,
			})
		}
	}

	// File-scoped and intelligent rules get a manual action note
	const scopedRules = project.rules.filter(
		(r) => r.ruleType === "file-scoped" || r.ruleType === "intelligent",
	)
	if (scopedRules.length > 0) {
		report.manualActions.push(
			`${scopedRules.length} file-scoped/intelligent rules found in ${project.path}. ` +
				`Devo does not support file-scoped rules natively. ` +
				`Consider merging their content into AGENTS.md or using path-conditional AGENTS.md files in subdirectories.`,
		)
	}

	// Agents
	for (const agent of project.agents) {
		const { content, report: agentReport } = convertAgentToOC(agent, options?.modelOverrides)
		result.agents.set(`${project.path}/.devo/agents/${agent.name}.md`, content)
		report.converted.push(...agentReport.converted)
	}

	// Commands
	for (const cmd of project.commands) {
		const { content, report: cmdReport } = convertCommandToOC(cmd)
		result.commands.set(`${project.path}/.devo/commands/${cmd.name}.md`, content)
		report.converted.push(...cmdReport.converted)
	}

	return { report }
}

// ============================================================
// MCP
// ============================================================

function convertMcpToOC(server: CanonicalMcpServer): Record<string, unknown> {
	if (server.type === "remote") {
		const result: Record<string, unknown> = {
			type: "remote",
			url: server.url,
		}
		if (server.headers) result.headers = server.headers
		if (server.enabled === false) result.enabled = false
		if (server.oauth) result.oauth = server.oauth
		return result
	}

	// Local
	const command: string[] = []
	if (server.command) command.push(server.command)
	if (server.args) command.push(...server.args)

	const result: Record<string, unknown> = {
		type: "local",
		command,
	}
	if (server.env && Object.keys(server.env).length > 0) {
		result.environment = server.env
	}
	if (server.enabled === false) result.enabled = false
	return result
}

// ============================================================
// Agents
// ============================================================

function convertAgentToOC(
	agent: CanonicalAgentFile,
	modelOverrides?: Record<string, string>,
): { content: string; report: ConversionReport } {
	const report = createEmptyReport()
	const fm: Record<string, unknown> = {}

	// Description
	fm.description = agent.description ?? agent.name

	// Mode
	fm.mode = agent.mode ?? inferMode(agent.name, agent.description)

	// Model
	if (agent.model && agent.model !== "inherit") {
		fm.model = translateModelId(agent.model, undefined, modelOverrides)
	}

	// Temperature
	fm.temperature = agent.temperature ?? inferTemperature(agent.name, agent.description)

	// Steps
	fm.steps = agent.maxSteps ?? (fm.mode === "subagent" ? 25 : 50)

	// Color
	if (agent.color) fm.color = agent.color

	const content = serializeFrontmatter(fm, agent.body)

	report.converted.push({
		category: "agents",
		source: agent.path,
		target: `${agent.name}.md`,
		details: `mode=${fm.mode}, temperature=${fm.temperature}`,
	})

	return { content, report }
}

function inferMode(name: string, description?: string): "primary" | "subagent" {
	const text = `${name} ${description ?? ""}`.toLowerCase()
	const primaryKeywords = [
		"build",
		"implement",
		"create",
		"develop",
		"main",
		"primary",
		"default",
		"general",
		"full",
		"orchestrat",
	]
	const subagentKeywords = [
		"review",
		"audit",
		"analyze",
		"check",
		"helper",
		"search",
		"find",
		"explore",
		"scan",
		"inspect",
		"verify",
	]

	for (const kw of primaryKeywords) if (text.includes(kw)) return "primary"
	for (const kw of subagentKeywords) if (text.includes(kw)) return "subagent"
	return "primary"
}

function inferTemperature(name: string, description?: string): number {
	const text = `${name} ${description ?? ""}`.toLowerCase()
	if (/security|audit|review|lint|check|verify|validate|test/.test(text)) return 0.1
	if (/code|implement|build|develop|engineer|refactor|fix|debug/.test(text)) return 0.3
	if (/document|write|explain|create|design|architect|plan/.test(text)) return 0.5
	return 0.3
}

// ============================================================
// Commands
// ============================================================

function convertCommandToOC(cmd: CanonicalCommandFile): {
	content: string
	report: ConversionReport
} {
	const report = createEmptyReport()

	const fm: DevoCommandFrontmatter = {
		description: cmd.description ?? cmd.name,
		agent: "build",
		subtask: false,
	}

	const content = serializeFrontmatter(fm as unknown as Record<string, unknown>, cmd.body)

	report.converted.push({
		category: "commands",
		source: cmd.path,
		target: `${cmd.name}.md`,
	})

	return { content, report }
}

// ============================================================
// Helpers
// ============================================================

function combineRulesForAgentsMd(rules: CanonicalRulesFile[]): string | undefined {
	const sections: string[] = []

	for (const rule of rules) {
		// Extract body content (skip frontmatter if present)
		const body = extractBody(rule.content)
		if (body) {
			sections.push(body)
		}
	}

	if (sections.length === 0) return undefined
	return sections.join("\n\n")
}

function extractBody(content: string): string {
	const match = content.match(/^---\r?\n[\s\S]*?\r?\n---\r?\n?([\s\S]*)$/)
	if (match) return match[1].trim()
	return content.trim()
}

function buildProviderConfig(
	env: Record<string, string>,
	report: ConversionReport,
): Record<string, { options?: Record<string, unknown> }> {
	const providers: Record<string, { options?: Record<string, unknown> }> = {}

	if (env.CLAUDE_CODE_USE_BEDROCK === "1") {
		providers["amazon-bedrock"] = { options: {} }
		report.converted.push({
			category: "config",
			source: "CLAUDE_CODE_USE_BEDROCK=1",
			target: 'provider: "amazon-bedrock"',
		})
	}

	if (env.CLAUDE_CODE_USE_VERTEX === "1") {
		providers["google-vertex"] = { options: {} }
		report.converted.push({
			category: "config",
			source: "CLAUDE_CODE_USE_VERTEX=1",
			target: 'provider: "google-vertex"',
		})
	}

	if (env.ANTHROPIC_API_KEY) {
		providers.anthropic = {
			options: { apiKey: "{env:ANTHROPIC_API_KEY}" },
		}
	}

	return providers
}
