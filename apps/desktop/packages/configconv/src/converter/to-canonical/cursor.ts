/**
 * Cursor -> Canonical format converter.
 *
 * Reads a Cursor ScanResult and produces a CanonicalScanResult.
 */
import type {
	CanonicalAgentFile,
	CanonicalCommandFile,
	CanonicalGlobalConfig,
	CanonicalMcpServer,
	CanonicalPermissions,
	CanonicalProjectConfig,
	CanonicalRulesFile,
	CanonicalScanResult,
	CanonicalSkillInfo,
} from "../../types/canonical"
import type {
	CursorAgentFile,
	CursorCommandFile,
	CursorMcpServer,
	CursorRule,
	CursorScanResult,
	CursorSkillInfo,
} from "../../types/cursor"
import { determineCursorRuleMode } from "../../types/cursor"

/**
 * Convert a Cursor ScanResult to canonical format.
 */
export function cursorToCanonical(scan: CursorScanResult): CanonicalScanResult {
	return {
		sourceFormat: "cursor",
		global: convertGlobal(scan),
		projects: scan.projects.map(convertProject),
	}
}

function convertGlobal(scan: CursorScanResult): CanonicalGlobalConfig {
	const global: CanonicalGlobalConfig = {
		mcpServers: {},
		skills: scan.global.skills.map(convertSkill),
		commands: scan.global.commands.map(convertCursorCommand),
		agents: scan.global.agents.map(convertCursorAgent),
	}

	// Global MCP servers
	if (scan.global.mcpJson?.mcpServers) {
		for (const [name, server] of Object.entries(scan.global.mcpJson.mcpServers)) {
			global.mcpServers[name] = convertCursorMcpServer(server)
		}
	}

	// CLI config permissions
	if (scan.global.cliConfig?.permissions) {
		global.permissions = convertCursorPermissions(scan.global.cliConfig.permissions)
	}

	return global
}

function convertProject(project: CursorProjectScanResult): CanonicalProjectConfig {
	const result: CanonicalProjectConfig = {
		path: project.path,
		mcpServers: {},
		rules: [],
		skills: project.skills.map(convertSkill),
		commands: project.commands.map(convertCursorCommand),
		agents: project.agents.map(convertCursorAgent),
	}

	// Project MCP servers
	if (project.mcpJson?.mcpServers) {
		for (const [name, server] of Object.entries(project.mcpJson.mcpServers)) {
			result.mcpServers[name] = convertCursorMcpServer(server)
		}
	}

	// Rules (.cursor/rules/*.mdc)
	for (const rule of project.rules) {
		result.rules.push(convertCursorRule(rule))
	}

	// AGENTS.md at project root
	if (project.agentsMd) {
		result.rules.push({
			path: project.agentsMdPath ?? `${project.path}/AGENTS.md`,
			name: "AGENTS.md",
			content: project.agentsMd,
			alwaysApply: true,
			ruleType: "always",
		})
	}

	// .cursorrules (legacy)
	if (project.cursorRules) {
		result.rules.push({
			path: project.cursorRulesPath ?? `${project.path}/.cursorrules`,
			name: ".cursorrules",
			content: project.cursorRules,
			alwaysApply: true,
			ruleType: "always",
		})
	}

	return result
}

import type { CursorProjectScanResult } from "../../types/cursor"

function convertCursorMcpServer(server: CursorMcpServer): CanonicalMcpServer {
	// Remote server: has url, no command
	if (server.url && !server.command) {
		const result: CanonicalMcpServer = {
			type: "remote",
			url: server.url,
			headers: server.headers,
		}
		if (server.auth) {
			result.oauth = server.auth as unknown as Record<string, unknown>
		}
		return result
	}

	// Local server: has command
	return {
		type: "local",
		command: server.command,
		args: server.args,
		env: server.env,
	}
}

function convertCursorRule(rule: CursorRule): CanonicalRulesFile {
	const mode = determineCursorRuleMode(rule.frontmatter)

	return {
		path: rule.path,
		name: rule.name,
		content: rule.content,
		alwaysApply: rule.frontmatter.alwaysApply === true,
		globs: normalizeGlobs(rule.frontmatter.globs),
		description: rule.frontmatter.description ?? undefined,
		ruleType: mode,
	}
}

function normalizeGlobs(globs: string | string[] | null | undefined): string | undefined {
	if (!globs) return undefined
	if (Array.isArray(globs)) return globs.join(",")
	return globs
}

function convertCursorPermissions(perms: {
	allow?: string[]
	deny?: string[]
}): CanonicalPermissions {
	const result: CanonicalPermissions = { "*": "ask" }

	const toolMap: Record<string, string> = {
		Shell: "bash",
		Read: "read",
		Write: "write",
		Edit: "edit",
	}

	const parsePattern = (raw: string) => {
		const match = raw.match(/^(\w+)\((.+)\)$/)
		if (match) return { tool: match[1], pattern: match[2] }
		if (/^\w+$/.test(raw)) return { tool: raw, pattern: "*" }
		return null
	}

	const isSafeKey = (key: string): boolean =>
		key !== "__proto__" && key !== "constructor" && key !== "prototype"

	const processPatterns = (patterns: string[], action: "allow" | "deny" | "ask") => {
		for (const raw of patterns) {
			const parsed = parsePattern(raw)
			if (!parsed) continue
			if (!isSafeKey(parsed.pattern)) continue
			const canonical = toolMap[parsed.tool] ?? parsed.tool.toLowerCase()

			if (parsed.pattern === "*") {
				result[canonical] = action
			} else {
				const existing = result[canonical]
				if (typeof existing === "object") {
					existing[parsed.pattern] = action
				} else if (typeof existing === "string") {
					result[canonical] = { "*": existing, [parsed.pattern]: action }
				} else {
					result[canonical] = { [parsed.pattern]: action }
				}
			}
		}
	}

	if (perms.allow) processPatterns(perms.allow, "allow")
	if (perms.deny) processPatterns(perms.deny, "deny")

	return result
}

function convertSkill(skill: CursorSkillInfo): CanonicalSkillInfo {
	return {
		path: skill.path,
		name: skill.name,
		description: skill.description,
		isSymlink: skill.isSymlink,
		symlinkTarget: skill.symlinkTarget,
	}
}

function convertCursorAgent(agent: CursorAgentFile): CanonicalAgentFile {
	return {
		path: agent.path,
		name: agent.name,
		content: agent.content,
		frontmatter: agent.frontmatter as Record<string, unknown>,
		body: agent.body,
		description: agent.frontmatter.description,
	}
}

function convertCursorCommand(cmd: CursorCommandFile): CanonicalCommandFile {
	return {
		path: cmd.path,
		name: cmd.name,
		content: cmd.content,
		frontmatter: {},
		body: cmd.body,
	}
}
