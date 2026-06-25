// @ts-nocheck
/**
 * Devo -> Canonical format converter.
 *
 * Reads Devo configuration files and produces a CanonicalScanResult.
 * This enables converting FROM Devo TO other formats.
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
import type { DevoConfig } from "../../types/devo"
import type { AgentFile, CommandFile, SkillInfo } from "../../types/scan-result"

/** Devo scan result (lightweight -- just the configs we discover) */
export interface DevoScanResult {
	global: DevoGlobalScanResult
	projects: DevoProjectScanResult[]
}

export interface DevoGlobalScanResult {
	/** ~/.config/devo/devo.json parsed */
	config?: Partial<DevoConfig>
	configPath?: string
	/** ~/.config/devo/AGENTS.md */
	agentsMd?: string
	agentsMdPath?: string
	/** Global agents */
	agents: AgentFile[]
	/** Global commands */
	commands: CommandFile[]
	/** Global skills */
	skills: SkillInfo[]
}

export interface DevoProjectScanResult {
	path: string
	/** devo.json at project root */
	config?: Partial<DevoConfig>
	configPath?: string
	/** AGENTS.md at project root */
	agentsMd?: string
	agentsMdPath?: string
	/** .devo/agents/*.md */
	agents: AgentFile[]
	/** .devo/commands/*.md */
	commands: CommandFile[]
	/** .devo/skills/ */
	skills: SkillInfo[]
}

/**
 * Convert an Devo ScanResult to canonical format.
 */
export function openCodeToCanonical(scan: DevoScanResult): CanonicalScanResult {
	return {
		sourceFormat: "devo",
		global: convertGlobal(scan),
		projects: scan.projects.map(convertProject),
	}
}

function convertGlobal(scan: DevoScanResult): CanonicalGlobalConfig {
	const config = scan.global.config

	const global: CanonicalGlobalConfig = {
		model: config?.model,
		smallModel: config?.small_model,
		mcpServers: {},
		skills: scan.global.skills.map(convertSkill),
		commands: scan.global.commands.map(convertCommand),
		agents: scan.global.agents.map(convertAgent),
	}

	// MCP servers
	if (config?.mcp) {
		for (const [name, server] of Object.entries(config.mcp)) {
			if (!server || typeof server !== "object") continue
			global.mcpServers[name] = convertOCMcpServer(server)
		}
	}

	// Permissions
	if (config?.permission) {
		global.permissions = config.permission as CanonicalPermissions
	}

	// Auto-update
	if (config?.autoupdate !== undefined) {
		global.autoUpdate = config.autoupdate === true
	}

	// Global rules (AGENTS.md)
	if (scan.global.agentsMd) {
		global.rules = [
			{
				path: scan.global.agentsMdPath ?? "~/.config/devo/AGENTS.md",
				name: "AGENTS.md",
				content: scan.global.agentsMd,
				alwaysApply: true,
				ruleType: "always",
			},
		]
	}

	// Provider config as extra
	if (config?.provider) {
		global.extraSettings = { provider: config.provider }
	}

	return global
}

function convertProject(project: DevoProjectScanResult): CanonicalProjectConfig {
	const config = project.config

	const result: CanonicalProjectConfig = {
		path: project.path,
		model: config?.model,
		mcpServers: {},
		rules: [],
		skills: project.skills.map(convertSkill),
		commands: project.commands.map(convertCommand),
		agents: project.agents.map(convertAgent),
	}

	// MCP servers
	if (config?.mcp) {
		for (const [name, server] of Object.entries(config.mcp)) {
			if (!server || typeof server !== "object") continue
			result.mcpServers[name] = convertOCMcpServer(server)
		}
	}

	// Permissions
	if (config?.permission) {
		result.permissions = config.permission as CanonicalPermissions
	}

	// AGENTS.md
	if (project.agentsMd) {
		result.rules.push({
			path: project.agentsMdPath ?? `${project.path}/AGENTS.md`,
			name: "AGENTS.md",
			content: project.agentsMd,
			alwaysApply: true,
			ruleType: "always",
		})
	}

	return result
}

function convertOCMcpServer(server: Record<string, unknown>): CanonicalMcpServer {
	const type = server.type as string

	if (type === "remote") {
		return {
			type: "remote",
			url: server.url as string | undefined,
			headers: server.headers as Record<string, string> | undefined,
			enabled: server.enabled as boolean | undefined,
			oauth: server.oauth as Record<string, unknown> | undefined,
		}
	}

	// Local server
	const command = server.command as string[] | undefined
	return {
		type: "local",
		command: command?.[0],
		args: command?.slice(1),
		env: server.environment as Record<string, string> | undefined,
		enabled: server.enabled as boolean | undefined,
	}
}

function convertSkill(skill: SkillInfo): CanonicalSkillInfo {
	return {
		path: skill.path,
		name: skill.name,
		description: skill.description,
		isSymlink: skill.isSymlink,
		symlinkTarget: skill.symlinkTarget,
	}
}

function convertAgent(agent: AgentFile): CanonicalAgentFile {
	const fm = agent.frontmatter
	return {
		path: agent.path,
		name: agent.name,
		content: agent.content,
		frontmatter: fm,
		body: agent.body,
		description: fm.description as string | undefined,
		mode: fm.mode as "primary" | "subagent" | undefined,
		model: fm.model as string | undefined,
		temperature: fm.temperature as number | undefined,
		maxSteps: fm.steps as number | undefined,
		color: fm.color as string | undefined,
	}
}

function convertCommand(cmd: CommandFile): CanonicalCommandFile {
	return {
		path: cmd.path,
		name: cmd.name,
		content: cmd.content,
		frontmatter: cmd.frontmatter,
		body: cmd.body,
		description: cmd.frontmatter.description as string | undefined,
	}
}
// @ts-nocheck
