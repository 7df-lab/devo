/**
 * Canonical -> Cursor format converter.
 *
 * Produces Cursor-compatible configuration files from canonical representation.
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
import type { CursorMcpServer } from "../../types/cursor"
import * as paths from "../../utils/paths"
import { serializeFrontmatter } from "../../utils/yaml"

/**
 * Convert canonical scan result to Cursor output files.
 */
export function canonicalToCursor(scan: CanonicalScanResult): CanonicalConversionResult {
	const result: CanonicalConversionResult = {
		sourceFormat: scan.sourceFormat,
		targetFormat: "cursor",
		globalConfig: {},
		projectConfigs: new Map(),
		agents: new Map(),
		commands: new Map(),
		rules: new Map(),
		extraFiles: new Map(),
		report: createEmptyReport(),
	}

	const reports: ConversionReport[] = []

	// ─── Global MCP -> ~/.cursor/mcp.json ────────────────────────────
	if (Object.keys(scan.global.mcpServers).length > 0) {
		const { mcpJson, report } = convertMcpToCursor(scan.global.mcpServers, "global")
		result.globalConfig = mcpJson
		reports.push(report)
	}

	// ─── Global agents -> ~/.cursor/agents/ ──────────────────────────
	for (const agent of scan.global.agents) {
		const { content, report } = convertAgentToCursor(agent)
		result.agents.set(`${paths.cursorGlobalAgentsDir()}/${agent.name}.md`, content)
		reports.push(report)
	}

	// ─── Global commands -> ~/.cursor/commands/ ──────────────────────
	for (const cmd of scan.global.commands) {
		const { content, report } = convertCommandToCursor(cmd)
		result.commands.set(`${paths.cursorGlobalCommandsDir()}/${cmd.name}.md`, content)
		reports.push(report)
	}

	// ─── Per-project conversion ──────────────────────────────────────
	for (const project of scan.projects) {
		const { report: projectReport } = convertProjectToCursor(project, result)
		reports.push(projectReport)
	}

	result.report = mergeReports(...reports)
	return result
}

function convertProjectToCursor(
	project: CanonicalProjectConfig,
	result: CanonicalConversionResult,
): { report: ConversionReport } {
	const report = createEmptyReport()

	// MCP servers -> .cursor/mcp.json
	if (Object.keys(project.mcpServers).length > 0) {
		const { mcpJson, report: mcpReport } = convertMcpToCursor(project.mcpServers, project.path)
		result.projectConfigs.set(project.path, mcpJson)
		report.converted.push(...mcpReport.converted)
		report.warnings.push(...mcpReport.warnings)
	}

	// Rules -> .cursor/rules/*.mdc
	for (const rule of project.rules) {
		const { path, content, report: ruleReport } = convertRuleToCursorMdc(rule, project.path)
		result.rules.set(path, content)
		report.converted.push(...ruleReport.converted)
	}

	// Agents -> .cursor/agents/*.md
	for (const agent of project.agents) {
		const { content, report: agentReport } = convertAgentToCursor(agent)
		result.agents.set(`${paths.cursorProjectAgentsDir(project.path)}/${agent.name}.md`, content)
		report.converted.push(...agentReport.converted)
	}

	// Commands -> .cursor/commands/*.md
	for (const cmd of project.commands) {
		const { content, report: cmdReport } = convertCommandToCursor(cmd)
		result.commands.set(`${paths.cursorProjectCommandsDir(project.path)}/${cmd.name}.md`, content)
		report.converted.push(...cmdReport.converted)
	}

	return { report }
}

// ============================================================
// MCP
// ============================================================

function convertMcpToCursor(
	servers: Record<string, CanonicalMcpServer>,
	source: string,
): { mcpJson: Record<string, unknown>; report: ConversionReport } {
	const report = createEmptyReport()
	const mcpServers: Record<string, CursorMcpServer> = {}

	for (const [name, server] of Object.entries(servers)) {
		if (server.type === "remote") {
			const cursorServer: CursorMcpServer = {
				url: server.url,
			}
			if (server.headers && Object.keys(server.headers).length > 0) {
				cursorServer.headers = { ...server.headers }
			}
			if (server.oauth) {
				cursorServer.auth = {
					CLIENT_ID: (server.oauth.clientId as string) ?? "",
					...(server.oauth.clientSecret
						? { CLIENT_SECRET: server.oauth.clientSecret as string }
						: {}),
					...(server.oauth.scopes ? { scopes: server.oauth.scopes as string[] } : {}),
				}
			}
			mcpServers[name] = cursorServer
		} else {
			// Local server
			const cursorServer: CursorMcpServer = {
				command: server.command ?? "",
				args: server.args,
			}
			if (server.env && Object.keys(server.env).length > 0) {
				cursorServer.env = { ...server.env }
			}
			mcpServers[name] = cursorServer
		}

		report.converted.push({
			category: "mcp",
			source: `${source}: ${name}`,
			target: `cursor mcp: ${name}`,
			details: `${server.type} server`,
		})

		// Warn about embedded credentials in URLs
		if (server.url && /[?&](token|key|secret|api_key)=/i.test(server.url)) {
			report.warnings.push(
				`MCP server "${name}": URL contains embedded credentials. ` +
					`Consider using \${env:VAR} interpolation.`,
			)
		}
	}

	return {
		mcpJson: { mcpServers } as unknown as Record<string, unknown>,
		report,
	}
}

// ============================================================
// Rules
// ============================================================

function convertRuleToCursorMdc(
	rule: CanonicalRulesFile,
	projectPath: string,
): { path: string; content: string; report: ConversionReport } {
	const report = createEmptyReport()

	// Determine MDC frontmatter based on canonical rule type
	const frontmatter: Record<string, unknown> = {}

	if (rule.description) {
		frontmatter.description = rule.description
	}
	if (rule.globs) {
		frontmatter.globs = rule.globs
	}
	frontmatter.alwaysApply = rule.alwaysApply ?? false

	// For AGENTS.md / CLAUDE.md content, create an always-apply rule
	if (rule.ruleType === "always" && !rule.globs) {
		frontmatter.alwaysApply = true
	}

	const fileName = sanitizeRuleName(rule.name)
	const targetPath = `${paths.cursorProjectRulesDir(projectPath)}/${fileName}.mdc`
	const content = serializeFrontmatter(
		frontmatter,
		rule.content.includes("---\n") ? extractBody(rule.content) : rule.content,
	)

	report.converted.push({
		category: "rules",
		source: rule.path,
		target: targetPath,
		details: `Rule type: ${rule.ruleType ?? "general"}`,
	})

	return { path: targetPath, content, report }
}

function sanitizeRuleName(name: string): string {
	// Remove file extensions and sanitize for filesystem
	return name
		.replace(/\.(md|mdc|txt)$/, "")
		.replace(/[^a-zA-Z0-9_-]/g, "-")
		.toLowerCase()
}

function extractBody(content: string): string {
	// If content has frontmatter, extract just the body
	const match = content.match(/^---\r?\n[\s\S]*?\r?\n---\r?\n?([\s\S]*)$/)
	if (match) return match[1].trim()
	return content.trim()
}

// ============================================================
// Agents
// ============================================================

function convertAgentToCursor(agent: CanonicalAgentFile): {
	content: string
	report: ConversionReport
} {
	const report = createEmptyReport()

	// Cursor agent frontmatter is minimal: name + description
	const frontmatter: Record<string, unknown> = {}
	if (agent.name) frontmatter.name = agent.name
	if (agent.description) frontmatter.description = agent.description

	const content =
		Object.keys(frontmatter).length > 0 ? serializeFrontmatter(frontmatter, agent.body) : agent.body

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

function convertCommandToCursor(cmd: CanonicalCommandFile): {
	content: string
	report: ConversionReport
} {
	const report = createEmptyReport()

	// Cursor commands are plain markdown (no frontmatter)
	const content = cmd.body

	report.converted.push({
		category: "commands",
		source: cmd.path,
		target: `${cmd.name}.md`,
	})

	return { content, report }
}
