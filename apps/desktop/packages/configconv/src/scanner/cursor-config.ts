/**
 * Scanner for Cursor IDE configuration files.
 *
 * Discovers:
 * - ~/.cursor/mcp.json (global MCP servers)
 * - ~/.cursor/cli-config.json (CLI agent permissions)
 * - ~/.cursor/skills/ (user-level skills)
 * - ~/.cursor/commands/ (user-level commands)
 * - ~/.cursor/agents/ (user-level agents)
 * - .cursor/mcp.json (project MCP servers)
 * - .cursor/rules/*.mdc (project rules)
 * - .cursor/agents/*.md (project agents)
 * - .cursor/commands/*.md (project commands)
 * - .cursor/skills/ (project skills)
 * - .cursorrules (legacy rules)
 * - AGENTS.md (project root)
 */
import type {
	CursorAgentFile,
	CursorCliConfig,
	CursorCommandFile,
	CursorGlobalScanResult,
	CursorMcpJson,
	CursorProjectScanResult,
	CursorRule,
	CursorSkillInfo,
} from "../types/cursor"
import { exists, getSymlinkInfo, safeReadDir, safeReadFile, safeReadJson } from "../utils/fs"
import * as paths from "../utils/paths"
import { parseFrontmatter } from "../utils/yaml"

/**
 * Scan global Cursor configuration.
 */
export async function scanCursorGlobal(): Promise<CursorGlobalScanResult> {
	const result: CursorGlobalScanResult = {
		skills: [],
		commands: [],
		agents: [],
	}

	// ~/.cursor/mcp.json
	const mcpJsonPath = paths.cursorGlobalMcpJsonPath()
	const mcpJson = await safeReadJson<CursorMcpJson>(mcpJsonPath)
	if (mcpJson) {
		result.mcpJson = mcpJson
		result.mcpJsonPath = mcpJsonPath
	}

	// ~/.cursor/cli-config.json
	const cliConfigPath = paths.cursorCliConfigPath()
	const cliConfig = await safeReadJson<CursorCliConfig>(cliConfigPath)
	if (cliConfig) {
		result.cliConfig = cliConfig
		result.cliConfigPath = cliConfigPath
	}

	// Global skills
	result.skills = await scanCursorSkillsDir(paths.cursorGlobalSkillsDir())

	// Global commands
	result.commands = await scanCursorCommandsDir(paths.cursorGlobalCommandsDir())

	// Global agents
	result.agents = await scanCursorAgentsDir(paths.cursorGlobalAgentsDir())

	return result
}

/**
 * Scan a specific project for Cursor configuration.
 */
export async function scanCursorProject(projectPath: string): Promise<CursorProjectScanResult> {
	const result: CursorProjectScanResult = {
		path: projectPath,
		rules: [],
		agents: [],
		commands: [],
		skills: [],
	}

	// .cursor/mcp.json
	const mcpJsonPath = paths.cursorProjectMcpJsonPath(projectPath)
	const mcpJson = await safeReadJson<CursorMcpJson>(mcpJsonPath)
	if (mcpJson) {
		result.mcpJson = mcpJson
		result.mcpJsonPath = mcpJsonPath
	}

	// .cursor/rules/*.mdc and *.md
	result.rules = await scanCursorRulesDir(paths.cursorProjectRulesDir(projectPath))

	// .cursor/agents/*.md
	result.agents = await scanCursorAgentsDir(paths.cursorProjectAgentsDir(projectPath))

	// .cursor/commands/*.md
	result.commands = await scanCursorCommandsDir(paths.cursorProjectCommandsDir(projectPath))

	// .cursor/skills/
	result.skills = await scanCursorSkillsDir(paths.cursorProjectSkillsDir(projectPath))

	// AGENTS.md
	const agentsMdPath = paths.projectAgentsMdPath(projectPath)
	const agentsMd = await safeReadFile(agentsMdPath)
	if (agentsMd) {
		result.agentsMd = agentsMd
		result.agentsMdPath = agentsMdPath
	}

	// .cursorrules (legacy)
	const cursorRulesPath = paths.cursorProjectLegacyRulesPath(projectPath)
	const cursorRules = await safeReadFile(cursorRulesPath)
	if (cursorRules) {
		result.cursorRules = cursorRules
		result.cursorRulesPath = cursorRulesPath
	}

	return result
}

// ─── Helpers ─────────────────────────────────────────────────────────

/**
 * Scan .cursor/rules/ for .mdc and .md rule files.
 */
async function scanCursorRulesDir(dir: string): Promise<CursorRule[]> {
	if (!(await exists(dir))) return []

	const entries = await scanDirRecursive(dir, [".mdc", ".md"])
	const rules: CursorRule[] = []

	for (const filePath of entries) {
		const content = await safeReadFile(filePath)
		if (!content) continue

		const { frontmatter, body } = parseFrontmatter(content)
		const name = filePath
			.split("/")
			.pop()!
			.replace(/\.(mdc|md)$/, "")

		rules.push({
			path: filePath,
			name,
			content,
			frontmatter: {
				description: (frontmatter.description as string | null) ?? undefined,
				globs: (frontmatter.globs as string | string[] | null) ?? undefined,
				alwaysApply: frontmatter.alwaysApply === true,
			},
			body,
		})
	}

	return rules
}

/**
 * Scan a directory for Cursor agent markdown files.
 */
async function scanCursorAgentsDir(dir: string): Promise<CursorAgentFile[]> {
	if (!(await exists(dir))) return []

	const entries = await scanDirRecursive(dir, [".md"])
	const agents: CursorAgentFile[] = []

	for (const filePath of entries) {
		const content = await safeReadFile(filePath)
		if (!content) continue

		const { frontmatter, body } = parseFrontmatter(content)
		const name = filePath.split("/").pop()!.replace(/\.md$/, "")

		agents.push({
			path: filePath,
			name,
			content,
			frontmatter: {
				name: (frontmatter.name as string) ?? undefined,
				description: (frontmatter.description as string) ?? undefined,
			},
			body,
		})
	}

	return agents
}

/**
 * Scan a directory for Cursor command markdown files.
 * Cursor commands are plain markdown (no frontmatter).
 */
async function scanCursorCommandsDir(dir: string): Promise<CursorCommandFile[]> {
	if (!(await exists(dir))) return []

	const entries = await scanDirRecursive(dir, [".md"])
	const commands: CursorCommandFile[] = []

	for (const filePath of entries) {
		const content = await safeReadFile(filePath)
		if (!content) continue

		const name = filePath.split("/").pop()!.replace(/\.md$/, "")

		commands.push({
			path: filePath,
			name,
			content,
			body: content.trim(),
		})
	}

	return commands
}

/**
 * Scan a skills directory for SKILL.md files.
 */
async function scanCursorSkillsDir(dir: string): Promise<CursorSkillInfo[]> {
	if (!(await exists(dir))) return []

	const entries = await safeReadDir(dir)
	const skills: CursorSkillInfo[] = []

	for (const entry of entries) {
		// Skip internal skills directory
		if (entry === "skills-cursor") continue

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

/**
 * Recursively scan a directory for files with given extensions.
 */
async function scanDirRecursive(dir: string, extensions: string[]): Promise<string[]> {
	try {
		const { readdir } = await import("node:fs/promises")
		const { join } = await import("node:path")
		const entries = await readdir(dir, { recursive: true, withFileTypes: true })
		return entries
			.filter((e) => e.isFile() && extensions.some((ext) => e.name.endsWith(ext)))
			.map((e) => join(e.parentPath ?? dir, e.name))
	} catch {
		return []
	}
}
