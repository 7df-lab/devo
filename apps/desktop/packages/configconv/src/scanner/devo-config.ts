/**
 * Scanner for Devo configuration files.
 *
 * Discovers:
 * - ~/.config/devo/devo.json (global config)
 * - ~/.config/devo/AGENTS.md (global rules)
 * - ~/.config/devo/agents/*.md (global agents)
 * - ~/.config/devo/commands/*.md (global commands)
 * - ~/.config/devo/skills/ (global skills)
 * - devo.json (project config)
 * - AGENTS.md (project root)
 * - .devo/agents/*.md (project agents)
 * - .devo/commands/*.md (project commands)
 * - .devo/skills/ (project skills)
 */

import type {
	DevoGlobalScanResult,
	DevoProjectScanResult,
} from "../converter/to-canonical/devo"
import type { DevoConfig } from "../types/devo"
import type { AgentFile, CommandFile, SkillInfo } from "../types/scan-result"
import { exists, getSymlinkInfo, globDir, safeReadDir, safeReadFile } from "../utils/fs"
import { parseJsonc } from "../utils/json"
import * as paths from "../utils/paths"
import { parseFrontmatter } from "../utils/yaml"

/**
 * Scan global Devo configuration.
 */
export async function scanDevoGlobal(): Promise<DevoGlobalScanResult> {
	const result: DevoGlobalScanResult = {
		agents: [],
		commands: [],
		skills: [],
	}

	// ~/.config/devo/devo.json
	const configPath = paths.ocGlobalConfigPath()
	const configContent = await safeReadFile(configPath)
	if (configContent) {
		try {
			result.config = parseJsonc<Partial<DevoConfig>>(configContent)
			result.configPath = configPath
		} catch {
			// Skip malformed config
		}
	}

	// ~/.config/devo/AGENTS.md
	const agentsMdPath = paths.ocGlobalAgentsMdPath()
	const agentsMd = await safeReadFile(agentsMdPath)
	if (agentsMd) {
		result.agentsMd = agentsMd
		result.agentsMdPath = agentsMdPath
	}

	// Global agents
	result.agents = await scanMarkdownDir(paths.ocGlobalAgentsDir())

	// Global commands
	result.commands = await scanMarkdownDir(paths.ocGlobalCommandsDir())

	// Global skills
	result.skills = await scanSkillsDir(paths.ocGlobalSkillsDir())

	return result
}

/**
 * Scan a specific project for Devo configuration.
 */
export async function scanDevoProject(projectPath: string): Promise<DevoProjectScanResult> {
	const result: DevoProjectScanResult = {
		path: projectPath,
		agents: [],
		commands: [],
		skills: [],
	}

	// devo.json at project root
	const configPath = paths.ocProjectConfigPath(projectPath)
	const configContent = await safeReadFile(configPath)
	if (configContent) {
		try {
			result.config = parseJsonc<Partial<DevoConfig>>(configContent)
			result.configPath = configPath
		} catch {
			// Skip malformed config
		}
	}

	// AGENTS.md
	const agentsMdPath = paths.ocProjectAgentsMdPath(projectPath)
	const agentsMd = await safeReadFile(agentsMdPath)
	if (agentsMd) {
		result.agentsMd = agentsMd
		result.agentsMdPath = agentsMdPath
	}

	// .devo/agents/*.md
	result.agents = await scanMarkdownDir(paths.ocProjectAgentsDir(projectPath))

	// .devo/commands/*.md
	result.commands = await scanMarkdownDir(paths.ocProjectCommandsDir(projectPath))

	// .devo/skills/
	result.skills = await scanSkillsDir(paths.ocProjectSkillsDir(projectPath))

	return result
}

// ─── Helpers ─────────────────────────────────────────────────────────

async function scanMarkdownDir(dir: string): Promise<(AgentFile | CommandFile)[]> {
	if (!(await exists(dir))) return []

	const files = await globDir(dir, "**/*.md")
	const results: (AgentFile | CommandFile)[] = []

	for (const filePath of files) {
		const content = await safeReadFile(filePath)
		if (!content) continue

		const { frontmatter, body } = parseFrontmatter(content)
		const name = filePath.split("/").pop()!.replace(/\.md$/, "")

		results.push({ path: filePath, name, content, frontmatter, body })
	}

	return results
}

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
