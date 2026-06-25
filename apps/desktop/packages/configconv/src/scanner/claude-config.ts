/**
 * Scanner for global Claude Code configuration files.
 */
import type {
	ClaudeHistoryEntry,
	ClaudeMcpJson,
	ClaudeProjectSettings,
	ClaudeSessionIndex,
	ClaudeSettings,
	ClaudeUserState,
} from "../types/claude-code"
import type {
	AgentFile,
	CommandFile,
	GlobalScanResult,
	HistoryScanResult,
	ProjectScanResult,
	SkillInfo,
} from "../types/scan-result"
import {
	exists,
	getSymlinkInfo,
	globDir,
	readJsonl,
	safeReadDir,
	safeReadFile,
	safeReadJson,
} from "../utils/fs"
import { parseJsonc } from "../utils/json"
import * as paths from "../utils/paths"
import { parseFrontmatter } from "../utils/yaml"

/**
 * Scan global Claude Code configuration.
 */
export async function scanGlobal(): Promise<GlobalScanResult> {
	const result: GlobalScanResult = { skills: [] }

	// ~/.Claude/settings.json
	const settingsPath = paths.ccSettingsPath()
	const settingsContent = await safeReadFile(settingsPath)
	if (settingsContent) {
		try {
			result.settings = parseJsonc<ClaudeSettings>(settingsContent)
			result.settingsPath = settingsPath
		} catch {
			// Skip malformed settings
		}
	}

	// ~/.claude.json
	const userStatePath = paths.ccUserStatePath()
	const userStateContent = await safeReadFile(userStatePath)
	if (userStateContent) {
		try {
			result.userState = JSON.parse(userStateContent) as ClaudeUserState
			result.userStatePath = userStatePath
		} catch {
			// Skip malformed user state
		}
	}

	// Global skills
	result.skills = await scanSkillsDir(paths.ccGlobalSkillsDir())
	const sharedSkills = await scanSkillsDir(paths.sharedAgentsSkillsDir())
	// Merge, deduplicate by name
	const seenNames = new Set(result.skills.map((s) => s.name))
	for (const skill of sharedSkills) {
		if (!seenNames.has(skill.name)) {
			result.skills.push(skill)
			seenNames.add(skill.name)
		}
	}

	// Global CLAUDE.md
	const claudeMdPath = paths.ccGlobalClaudeMdPath()
	const claudeMd = await safeReadFile(claudeMdPath)
	if (claudeMd) {
		result.claudeMd = claudeMd
		result.claudeMdPath = claudeMdPath
	}

	return result
}

/**
 * Scan a specific project for Claude Code configuration.
 */
export async function scanProject(
	projectPath: string,
	userState?: ClaudeUserState,
): Promise<ProjectScanResult> {
	const result: ProjectScanResult = {
		path: projectPath,
		agents: [],
		commands: [],
		skills: [],
		projectMcpServers: {},
	}

	// .claude/settings.local.json
	const settingsLocalPath = paths.ccProjectSettingsPath(projectPath)
	const settingsLocal = await safeReadJson<ClaudeProjectSettings>(settingsLocalPath)
	if (settingsLocal) {
		result.settingsLocal = settingsLocal
		result.settingsLocalPath = settingsLocalPath
	}

	// .mcp.json
	const mcpJsonPath = paths.ccProjectMcpJsonPath(projectPath)
	const mcpJson = await safeReadJson<ClaudeMcpJson>(mcpJsonPath)
	if (mcpJson) {
		result.mcpJson = mcpJson
		result.mcpJsonPath = mcpJsonPath
	}

	// .claude/agents/*.md
	result.agents = await scanMarkdownDir(paths.ccProjectAgentsDir(projectPath))

	// .claude/commands/*.md (recursive)
	result.commands = await scanMarkdownDir(paths.ccProjectCommandsDir(projectPath))

	// .claude/skills/
	result.skills = await scanSkillsDir(paths.ccProjectSkillsDir(projectPath))

	// CLAUDE.md
	const claudeMdPath = paths.ccProjectClaudeMdPath(projectPath)
	const claudeMd = await safeReadFile(claudeMdPath)
	if (claudeMd) {
		result.claudeMd = claudeMd
		result.claudeMdPath = claudeMdPath
	}

	// AGENTS.md
	const agentsMdPath = paths.projectAgentsMdPath(projectPath)
	const agentsMd = await safeReadFile(agentsMdPath)
	if (agentsMd) {
		result.agentsMd = agentsMd
		result.agentsMdPath = agentsMdPath
	}

	// Extract per-project data from ~/.claude.json
	if (userState?.projects) {
		const projectEntry = userState.projects[projectPath]
		if (projectEntry) {
			result.projectMcpServers = projectEntry.mcpServers ?? {}
			result.allowedTools = projectEntry.allowedTools
			result.disabledMcpServers = projectEntry.disabledMcpjsonServers
			result.enabledMcpServers = projectEntry.enabledMcpjsonServers
			result.ignorePatterns = projectEntry.ignorePatterns
			result.trustAccepted = projectEntry.hasTrustDialogAccepted
		}
	}

	return result
}

/**
 * Scan session history across all projects.
 */
export async function scanHistory(since?: Date): Promise<HistoryScanResult> {
	const result: HistoryScanResult = {
		sessionIndices: [],
		totalSessions: 0,
		totalMessages: 0,
	}

	const projectsDir = paths.ccProjectsDir()
	if (!(await exists(projectsDir))) return result

	const dirs = await safeReadDir(projectsDir)
	for (const dir of dirs) {
		const indexPath = `${projectsDir}/${dir}/sessions-index.json`
		const index = await safeReadJson<ClaudeSessionIndex>(indexPath)
		if (!index?.entries) continue

		let entries = index.entries
		if (since) {
			entries = entries.filter((e) => {
				if (!e.created) return true
				return new Date(e.created) >= since
			})
		}

		const mangledPath = dir
		const projectPath = index.originalPath ?? mangledPath.replace(/^-/, "/").replace(/-/g, "/")

		result.sessionIndices.push({
			projectPath,
			mangledPath,
			index: { ...index, entries },
		})
		result.totalSessions += entries.length
		result.totalMessages += entries.reduce((sum, e) => sum + (e.messageCount ?? 0), 0)
	}

	// Prompt history
	const historyEntries = await readJsonl<ClaudeHistoryEntry>(paths.ccHistoryPath())
	if (historyEntries.length > 0) {
		result.promptHistory = historyEntries
			.filter((e) => !since || e.timestamp >= since.getTime())
			.map((e) => ({
				display: e.display,
				timestamp: e.timestamp,
				project: e.project,
			}))
	}

	return result
}

// ─── Helpers ─────────────────────────────────────────────────────────

/**
 * Scan a directory for markdown files with frontmatter.
 */
async function scanMarkdownDir(dir: string): Promise<(AgentFile | CommandFile)[]> {
	if (!(await exists(dir))) return []

	const files = await globDir(dir, "**/*.md")
	const results: (AgentFile | CommandFile)[] = []

	for (const filePath of files) {
		const content = await safeReadFile(filePath)
		if (!content) continue

		const { frontmatter, body } = parseFrontmatter(content)
		const name = filePath.split("/").pop()!.replace(/\.md$/, "")

		results.push({
			path: filePath,
			name,
			content,
			frontmatter,
			body,
		})
	}

	return results
}

/**
 * Scan a skills directory for SKILL.md files.
 */
async function scanSkillsDir(dir: string): Promise<SkillInfo[]> {
	if (!(await exists(dir))) return []

	const entries = await safeReadDir(dir)
	const skills: SkillInfo[] = []

	for (const entry of entries) {
		const skillDir = `${dir}/${entry}`
		const skillMdPath = `${skillDir}/SKILL.md`
		const content = await safeReadFile(skillMdPath)

		const symlinkInfo = await getSymlinkInfo(skillDir)

		if (content) {
			const { frontmatter } = parseFrontmatter(content)
			skills.push({
				path: skillMdPath,
				name: (frontmatter.name as string) ?? entry,
				description: frontmatter.description as string | undefined,
				isSymlink: symlinkInfo.isSymlink,
				symlinkTarget: symlinkInfo.target,
			})
		} else if (await exists(skillDir)) {
			// Directory exists but no SKILL.md
			skills.push({
				path: skillDir,
				name: entry,
				isSymlink: symlinkInfo.isSymlink,
				symlinkTarget: symlinkInfo.target,
			})
		}
	}

	return skills
}
