/**
 * Canonical -> Claude Code format converter.
 *
 * Produces Claude Code-compatible configuration files from canonical representation.
 */
import type {
	CanonicalAgentFile,
	CanonicalCommandFile,
	CanonicalConversionResult,
	CanonicalMcpServer,
	CanonicalPermissions,
	CanonicalProjectConfig,
	CanonicalRulesFile,
	CanonicalScanResult,
	ConversionReport,
} from "../../types/canonical"
import { createEmptyReport, mergeReports } from "../../types/canonical"
import type { ClaudeMcpServer, ClaudePermissions } from "../../types/claude-code"
import * as paths from "../../utils/paths"
import { serializeFrontmatter } from "../../utils/yaml"

/**
 * Convert canonical scan result to Claude Code output files.
 */
export function canonicalToClaudeCode(scan: CanonicalScanResult): CanonicalConversionResult {
	const result: CanonicalConversionResult = {
		sourceFormat: scan.sourceFormat,
		targetFormat: "claude-code",
		globalConfig: {},
		projectConfigs: new Map(),
		agents: new Map(),
		commands: new Map(),
		rules: new Map(),
		extraFiles: new Map(),
		report: createEmptyReport(),
	}

	const reports: ConversionReport[] = []

	// ─── Global config -> ~/.Claude/settings.json ────────────────────
	const { config: globalConfig, report: globalReport } = convertGlobalToCC(scan.global)
	result.globalConfig = globalConfig
	reports.push(globalReport)

	// ─── Global rules -> ~/.claude/CLAUDE.md ─────────────────────────
	if (scan.global.rules && scan.global.rules.length > 0) {
		const combined = combineRulesForClaudeMd(scan.global.rules)
		if (combined) {
			result.rules.set(paths.ccGlobalClaudeMdPath(), combined)
		}
	}

	// ─── Per-project conversion ──────────────────────────────────────
	for (const project of scan.projects) {
		const { report: projectReport } = convertProjectToCC(project, result)
		reports.push(projectReport)
	}

	result.report = mergeReports(...reports)
	return result
}

function convertGlobalToCC(global: CanonicalScanResult["global"]): {
	config: Record<string, unknown>
	report: ConversionReport
} {
	const report = createEmptyReport()
	const config: Record<string, unknown> = {}

	// Model (strip provider prefix if present)
	if (global.model) {
		config.model = stripProviderPrefix(global.model)
		report.converted.push({
			category: "config",
			source: `model: "${global.model}"`,
			target: `model: "${config.model}"`,
		})
	}

	// Permissions
	if (global.permissions) {
		config.permissions = convertToClaudePermissions(global.permissions)
		report.converted.push({
			category: "permissions",
			source: "global permissions",
			target: "permissions",
		})
	}

	// Environment
	if (global.env) {
		config.env = { ...global.env }
	}

	// Auto-update
	if (global.autoUpdate !== undefined) {
		config.autoUpdatesChannel = global.autoUpdate ? "latest" : undefined
	}

	return { config, report }
}

function convertProjectToCC(
	project: CanonicalProjectConfig,
	result: CanonicalConversionResult,
): { report: ConversionReport } {
	const report = createEmptyReport()

	// MCP servers -> .mcp.json
	if (Object.keys(project.mcpServers).length > 0) {
		const mcpServers: Record<string, ClaudeMcpServer> = {}
		for (const [name, server] of Object.entries(project.mcpServers)) {
			mcpServers[name] = convertMcpToCC(server)
			report.converted.push({
				category: "mcp",
				source: `${project.path}: ${name}`,
				target: `.mcp.json: ${name}`,
				details: `${server.type} server`,
			})
		}
		result.projectConfigs.set(project.path, { mcpServers })
	}

	// Rules -> CLAUDE.md
	const alwaysRules = project.rules.filter(
		(r) => r.ruleType === "always" || r.alwaysApply || r.ruleType === "general",
	)
	if (alwaysRules.length > 0) {
		const combined = combineRulesForClaudeMd(alwaysRules)
		if (combined) {
			result.rules.set(`${project.path}/CLAUDE.md`, combined)
			report.converted.push({
				category: "rules",
				source: `${alwaysRules.length} rules`,
				target: `${project.path}/CLAUDE.md`,
			})
		}
	}

	// File-scoped rules from Cursor get a warning
	const scopedRules = project.rules.filter(
		(r) => r.ruleType === "file-scoped" || r.ruleType === "intelligent",
	)
	if (scopedRules.length > 0) {
		report.manualActions.push(
			`${scopedRules.length} file-scoped/intelligent rules found in ${project.path}. ` +
				`Claude Code supports path-scoped rules via .claude/rules/*.md with \`paths\` frontmatter. ` +
				`These rules need manual adaptation.`,
		)
		// Write them as .claude/rules/*.md with paths frontmatter
		for (const rule of scopedRules) {
			const fm: Record<string, unknown> = {}
			if (rule.globs) {
				fm.paths = rule.globs.split(",").map((g) => g.trim())
			}
			const content = serializeFrontmatter(fm, extractBody(rule.content))
			result.rules.set(`${project.path}/.claude/rules/${sanitizeName(rule.name)}.md`, content)
		}
	}

	// Agents -> .claude/agents/*.md
	for (const agent of project.agents) {
		const { content, report: agentReport } = convertAgentToCC(agent)
		result.agents.set(`${paths.ccProjectAgentsDir(project.path)}/${agent.name}.md`, content)
		report.converted.push(...agentReport.converted)
	}

	// Commands -> .claude/commands/*.md
	for (const cmd of project.commands) {
		const { content, report: cmdReport } = convertCommandToCC(cmd)
		result.commands.set(`${paths.ccProjectCommandsDir(project.path)}/${cmd.name}.md`, content)
		report.converted.push(...cmdReport.converted)
	}

	return { report }
}

// ============================================================
// MCP
// ============================================================

function convertMcpToCC(server: CanonicalMcpServer): ClaudeMcpServer {
	if (server.type === "remote") {
		const result: ClaudeMcpServer = {
			type: server.url?.includes("/sse") ? "sse" : "http",
			url: server.url,
		}
		if (server.headers) result.headers = server.headers
		return result
	}

	// Local
	const result: ClaudeMcpServer = {
		command: server.command,
		args: server.args,
	}
	if (server.env && Object.keys(server.env).length > 0) {
		result.env = server.env
	}
	return result
}

// ============================================================
// Permissions
// ============================================================

function convertToClaudePermissions(canonical: CanonicalPermissions): ClaudePermissions {
	const result: ClaudePermissions = {
		allow: [],
		deny: [],
	}

	// Reverse tool name mapping
	const reverseToolMap: Record<string, string> = {
		read: "Read",
		write: "Write",
		edit: "Edit",
		bash: "Bash",
		glob: "Glob",
		grep: "Grep",
		webfetch: "WebFetch",
		websearch: "WebSearch",
		task: "Task",
		todoread: "TodoRead",
		todowrite: "TodoWrite",
		skill: "Skill",
	}

	for (const [tool, action] of Object.entries(canonical)) {
		if (tool === "*") {
			if (action === "allow") {
				result.defaultMode = "bypassPermissions"
			}
			continue
		}

		const ccTool = reverseToolMap[tool] ?? tool
		if (typeof action === "string") {
			if (action === "allow") result.allow?.push(ccTool)
			else if (action === "deny") result.deny?.push(ccTool)
		} else if (typeof action === "object") {
			for (const [pattern, act] of Object.entries(action)) {
				const entry = pattern === "*" ? ccTool : `${ccTool}(${pattern})`
				if (act === "allow") result.allow?.push(entry)
				else if (act === "deny") result.deny?.push(entry)
			}
		}
	}

	return result
}

// ============================================================
// Agents
// ============================================================

function convertAgentToCC(agent: CanonicalAgentFile): {
	content: string
	report: ConversionReport
} {
	const report = createEmptyReport()
	const fm: Record<string, unknown> = {}

	if (agent.name) fm.name = agent.name
	if (agent.description) fm.description = agent.description
	if (agent.model) fm.model = stripProviderPrefix(agent.model)
	if (agent.tools) fm.tools = agent.tools.join(", ")
	if (agent.color) fm.color = agent.color

	const content = Object.keys(fm).length > 0 ? serializeFrontmatter(fm, agent.body) : agent.body

	report.converted.push({
		category: "agents",
		source: agent.path,
		target: `${agent.name}.md`,
	})

	return { content, report }
}

// ============================================================
// Commands
// ============================================================

function convertCommandToCC(cmd: CanonicalCommandFile): {
	content: string
	report: ConversionReport
} {
	const report = createEmptyReport()

	const fm: Record<string, unknown> = {}
	if (cmd.description) fm.description = cmd.description

	const content = Object.keys(fm).length > 0 ? serializeFrontmatter(fm, cmd.body) : cmd.body

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

function stripProviderPrefix(model: string): string {
	// "anthropic/claude-opus-4-6" -> "claude-opus-4-6"
	// "amazon-bedrock/anthropic.claude-opus-4-6" -> keep as is (Bedrock format)
	if (model.startsWith("anthropic/")) {
		return model.replace("anthropic/", "")
	}
	return model
}

function combineRulesForClaudeMd(rules: CanonicalRulesFile[]): string | undefined {
	const sections: string[] = []
	for (const rule of rules) {
		const body = extractBody(rule.content)
		if (body) sections.push(body)
	}
	if (sections.length === 0) return undefined
	return sections.join("\n\n")
}

function extractBody(content: string): string {
	const match = content.match(/^---\r?\n[\s\S]*?\r?\n---\r?\n?([\s\S]*)$/)
	if (match) return match[1].trim()
	return content.trim()
}

function sanitizeName(name: string): string {
	return name
		.replace(/\.(md|mdc|txt)$/, "")
		.replace(/[^a-zA-Z0-9_-]/g, "-")
		.toLowerCase()
}
