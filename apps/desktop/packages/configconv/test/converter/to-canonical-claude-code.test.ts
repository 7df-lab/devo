/**
 * Tests for Claude Code -> Canonical conversion.
 */
import { describe, expect, test } from "bun:test"
import { claudeCodeToCanonical } from "../../src/converter/to-canonical/claude-code"
import type { ScanResult } from "../../src/types/scan-result"

function emptyCCScan(): ScanResult {
	return {
		global: { skills: [] },
		projects: [],
	}
}

describe("claudeCodeToCanonical", () => {
	test("converts empty scan result", () => {
		const result = claudeCodeToCanonical(emptyCCScan())

		expect(result.sourceFormat).toBe("claude-code")
		expect(result.projects).toHaveLength(0)
		expect(Object.keys(result.global.mcpServers)).toHaveLength(0)
	})

	test("converts model from settings", () => {
		const scan = emptyCCScan()
		scan.global.settings = { model: "claude-opus-4-6" }

		const result = claudeCodeToCanonical(scan)

		expect(result.global.model).toBe("claude-opus-4-6")
	})

	test("converts permissions", () => {
		const scan = emptyCCScan()
		scan.global.settings = {
			permissions: {
				allow: ["Bash(git *)"],
				deny: ["Read(.env)"],
				defaultMode: "bypassPermissions",
			},
		}

		const result = claudeCodeToCanonical(scan)

		expect(result.global.permissions).toBeDefined()
		expect(result.global.permissions?.["*"]).toBe("allow")
		expect(result.global.permissions?.bash).toBeDefined()
	})

	test("converts CLAUDE.md to rules", () => {
		const scan = emptyCCScan()
		scan.global.claudeMd = "# Instructions\n\nUse TypeScript."
		scan.global.claudeMdPath = "/home/.claude/CLAUDE.md"

		const result = claudeCodeToCanonical(scan)

		expect(result.global.rules).toHaveLength(1)
		expect(result.global.rules?.[0].name).toBe("CLAUDE.md")
		expect(result.global.rules?.[0].alwaysApply).toBe(true)
	})

	test("converts project MCP servers from multiple sources", () => {
		const scan = emptyCCScan()
		scan.projects.push({
			path: "/test/project",
			agents: [],
			commands: [],
			skills: [],
			projectMcpServers: {
				FromJson: {
					command: "npx",
					args: ["server"],
				},
			},
			mcpJson: {
				mcpServers: {
					FromMcpJson: {
						type: "sse",
						url: "https://example.com/sse",
					},
				},
			},
		})

		const result = claudeCodeToCanonical(scan)

		expect(result.projects[0].mcpServers.FromJson.type).toBe("local")
		expect(result.projects[0].mcpServers.FromMcpJson.type).toBe("remote")
	})

	test("converts agents with frontmatter", () => {
		const scan = emptyCCScan()
		scan.projects.push({
			path: "/test/project",
			agents: [
				{
					path: "/test/project/.claude/agents/build.md",
					name: "build",
					content:
						"---\nname: build\ndescription: Build agent\ntools: Read, Edit, Bash\nmodel: opus\n---\n\nBody.",
					frontmatter: {
						name: "build",
						description: "Build agent",
						tools: "Read, Edit, Bash",
						model: "opus",
					},
					body: "Body.",
				},
			],
			commands: [],
			skills: [],
			projectMcpServers: {},
		})

		const result = claudeCodeToCanonical(scan)

		expect(result.projects[0].agents).toHaveLength(1)
		expect(result.projects[0].agents[0].description).toBe("Build agent")
		expect(result.projects[0].agents[0].model).toBe("opus")
		expect(result.projects[0].agents[0].tools).toEqual(["Read", "Edit", "Bash"])
	})

	test("converts commands", () => {
		const scan = emptyCCScan()
		scan.projects.push({
			path: "/test/project",
			agents: [],
			commands: [
				{
					path: "/test/project/.claude/commands/lint.md",
					name: "lint",
					content: "---\ndescription: Run linter\n---\n\nRun the project linter.",
					frontmatter: { description: "Run linter" },
					body: "Run the project linter.",
				},
			],
			skills: [],
			projectMcpServers: {},
		})

		const result = claudeCodeToCanonical(scan)

		expect(result.projects[0].commands).toHaveLength(1)
		expect(result.projects[0].commands[0].name).toBe("lint")
		expect(result.projects[0].commands[0].description).toBe("Run linter")
	})

	test("converts skills", () => {
		const scan = emptyCCScan()
		scan.global.skills = [
			{
				path: "/home/.Claude/skills/pdf/SKILL.md",
				name: "pdf",
				description: "PDF tools",
				isSymlink: true,
				symlinkTarget: "../../.agents/skills/pdf",
			},
		]

		const result = claudeCodeToCanonical(scan)

		expect(result.global.skills).toHaveLength(1)
		expect(result.global.skills[0].name).toBe("pdf")
		expect(result.global.skills[0].isSymlink).toBe(true)
	})

	test("captures extra settings (teammateMode, hooks)", () => {
		const scan = emptyCCScan()
		scan.global.settings = {
			teammateMode: "tmux",
			outputStyle: "Explanatory",
		}

		const result = claudeCodeToCanonical(scan)

		expect(result.global.extraSettings?.teammateMode).toBe("tmux")
		expect(result.global.extraSettings?.outputStyle).toBe("Explanatory")
	})

	test("converts autoUpdatesChannel to autoUpdate boolean", () => {
		const scan = emptyCCScan()
		scan.global.settings = { autoUpdatesChannel: "latest" }

		const result = claudeCodeToCanonical(scan)

		expect(result.global.autoUpdate).toBe(true)
	})
})
