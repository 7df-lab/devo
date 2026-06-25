/**
 * Claude Code -> Canonical format converter.
 *
 * Reads a Claude Code ScanResult and produces a CanonicalScanResult.
 */
import type {
	CanonicalAgentFile,
	CanonicalCommandFile,
	CanonicalGlobalConfig,
	CanonicalMcpServer,
	CanonicalPermissions,
	CanonicalProjectConfig,
	CanonicalScanResult,
	CanonicalSkillInfo,
} from "../../types/canonical"
import type { ClaudeMcpServer, ClaudePermissions } from "../../types/claude-code"
import type { ScanResult } from "../../types/scan-result"

/**
 * Convert a Claude Code ScanResult to canonical format.
 */
export function claudeCodeToCanonical(scan: ScanResult): CanonicalScanResult {
	return {
		sourceFormat: "claude-code",
		global: convertGlobal(scan),
		projects: scan.projects.map((p) => convertProject(p, scan)),
	}
}

function convertGlobal(scan: ScanResult): CanonicalGlobalConfig {
	const settings = scan.global.settings

	const global: CanonicalGlobalConfig = {
		model: settings?.model,
		mcpServers: {},
		skills: scan.global.skills.map(convertSkill),
		commands: [],
		agents: [],
		env: settings?.env,
	}

	// Permissions
	if (settings?.permissions) {
		global.permissions = convertCCPermissions(settings.permissions)
	}

	// Auto-update
	if (settings?.autoUpdatesChannel) {
		global.autoUpdate = true
	}

	// Global rules (CLAUDE.md)
	if (scan.global.claudeMd) {
		global.rules = [
			{
				path: scan.global.claudeMdPath ?? "CLAUDE.md",
				name: "CLAUDE.md",
				content: scan.global.claudeMd,
				alwaysApply: true,
				ruleType: "always",
			},
		]
	}

	// Extra settings that don't map cleanly
	const extra: Record<string, unknown> = {}
	if (settings?.teammateMode) extra.teammateMode = settings.teammateMode
	if (settings?.hooks) extra.hooks = settings.hooks
	if (settings?.sandbox) extra.sandbox = settings.sandbox
	if (settings?.apiKeyHelper) extra.apiKeyHelper = settings.apiKeyHelper
	if (settings?.outputStyle) extra.outputStyle = settings.outputStyle
	if (Object.keys(extra).length > 0) {
		global.extraSettings = extra
	}

	return global
}

function convertProject(
	project: import("../../types/scan-result").ProjectScanResult,
	_scan: ScanResult,
): CanonicalProjectConfig {
	const result: CanonicalProjectConfig = {
		path: project.path,
		mcpServers: {},
		rules: [],
		skills: project.skills.map(convertSkill),
		commands: project.commands.map(convertCommand),
		agents: project.agents.map(convertAgent),
	}

	// Model override from project-level settings
	if (project.settingsLocal?.model) {
		result.model = project.settingsLocal.model
	}

	// MCP servers from multiple sources
	// 1. From .mcp.json
	if (project.mcpJson?.mcpServers) {
		for (const [name, server] of Object.entries(project.mcpJson.mcpServers)) {
			result.mcpServers[name] = convertCCMcpServer(server)
		}
	}
	// 2. From ~/.claude.json per-project mcpServers
	for (const [name, server] of Object.entries(project.projectMcpServers)) {
		result.mcpServers[name] = convertCCMcpServer(server)
	}
	// 3. From .claude/settings.local.json
	if (project.settingsLocal?.mcpServers) {
		for (const [name, server] of Object.entries(project.settingsLocal.mcpServers)) {
			result.mcpServers[name] = convertCCMcpServer(server)
		}
	}

	// Permissions
	if (project.settingsLocal?.permissions || project.allowedTools) {
		result.permissions = convertCCPermissions(
			project.settingsLocal?.permissions,
			project.allowedTools,
		)
	}

	// Rules (CLAUDE.md)
	if (project.claudeMd) {
		result.rules.push({
			path: project.claudeMdPath ?? `${project.path}/CLAUDE.md`,
			name: "CLAUDE.md",
			content: project.claudeMd,
			alwaysApply: true,
			ruleType: "always",
		})
	}

	// Disabled/enabled MCP servers
	result.disabledMcpServers = project.disabledMcpServers
	result.enabledMcpServers = project.enabledMcpServers
	result.ignorePatterns = project.ignorePatterns

	return result
}

function convertCCMcpServer(server: ClaudeMcpServer): CanonicalMcpServer {
	const isRemote =
		server.type === "sse" || server.type === "http" || (server.url && !server.command)

	if (isRemote) {
		return {
			type: "remote",
			url: server.url,
			headers: server.headers,
		}
	}

	return {
		type: "local",
		command: server.command,
		args: server.args,
		env: server.env,
	}
}

function convertCCPermissions(
	perms?: ClaudePermissions,
	allowedTools?: string[],
): CanonicalPermissions {
	const result: CanonicalPermissions = {}

	if (!perms && !allowedTools) {
		return { "*": "ask" }
	}

	// Default mode
	if (perms?.defaultMode === "bypassPermissions") {
		result["*"] = "allow"
	} else {
		result["*"] = "ask"
	}

	// Tool name mapping
	const toolMap: Record<string, string> = {
		Read: "read",
		Write: "write",
		Edit: "edit",
		MultiEdit: "edit",
		Bash: "bash",
		Glob: "glob",
		Grep: "grep",
		WebFetch: "webfetch",
		WebSearch: "websearch",
		Task: "task",
		TodoRead: "todoread",
		TodoWrite: "todowrite",
		Skill: "skill",
	}

	const parsePattern = (raw: string) => {
		const match = raw.match(/^(\w+)\((.+)\)$/)
		if (match) return { tool: match[1], pattern: match[2] }
		if (/^\w+$/.test(raw)) return { tool: raw, pattern: "*" }
		return null
	}

	const processPatterns = (patterns: string[], action: "allow" | "deny" | "ask") => {
		for (const raw of patterns) {
			const parsed = parsePattern(raw)
			if (!parsed) continue
			const ocTool = toolMap[parsed.tool]
			if (!ocTool) continue

			if (parsed.pattern === "*") {
				const existing = result[ocTool]
				if (typeof existing === "object") {
					existing["*"] = action
				} else {
					result[ocTool] = action
				}
			} else {
				const existing = result[ocTool]
				if (typeof existing === "object") {
					existing[parsed.pattern] = action
				} else if (typeof existing === "string") {
					result[ocTool] = { "*": existing, [parsed.pattern]: action }
				} else {
					result[ocTool] = { [parsed.pattern]: action }
				}
			}
		}
	}

	if (perms?.allow) processPatterns(perms.allow, "allow")
	if (perms?.deny) processPatterns(perms.deny, "deny")
	if (perms?.ask) processPatterns(perms.ask, "ask")
	if (allowedTools) processPatterns(allowedTools, "allow")

	return result
}

function convertSkill(skill: import("../../types/scan-result").SkillInfo): CanonicalSkillInfo {
	return {
		path: skill.path,
		name: skill.name,
		description: skill.description,
		isSymlink: skill.isSymlink,
		symlinkTarget: skill.symlinkTarget,
	}
}

function convertAgent(agent: import("../../types/scan-result").AgentFile): CanonicalAgentFile {
	const fm = agent.frontmatter
	return {
		path: agent.path,
		name: agent.name,
		content: agent.content,
		frontmatter: fm,
		body: agent.body,
		description: fm.description as string | undefined,
		model: fm.model as string | undefined,
		tools:
			typeof fm.tools === "string"
				? (fm.tools as string).split(",").map((t) => t.trim())
				: (fm.tools as string[] | undefined),
		color: fm.color as string | undefined,
	}
}

function convertCommand(cmd: import("../../types/scan-result").CommandFile): CanonicalCommandFile {
	return {
		path: cmd.path,
		name: cmd.name,
		content: cmd.content,
		frontmatter: cmd.frontmatter,
		body: cmd.body,
		description: cmd.frontmatter.description as string | undefined,
	}
}
