/**
 * Tests for Cursor -> Canonical conversion.
 */
import { describe, expect, test } from "bun:test"
import { cursorToCanonical } from "../../src/converter/to-canonical/cursor"
import type { CursorScanResult } from "../../src/types/cursor"

function emptyCursorScan(): CursorScanResult {
	return {
		global: { skills: [], commands: [], agents: [] },
		projects: [],
	}
}

describe("cursorToCanonical", () => {
	test("converts empty scan result", () => {
		const result = cursorToCanonical(emptyCursorScan())

		expect(result.sourceFormat).toBe("cursor")
		expect(result.projects).toHaveLength(0)
		expect(Object.keys(result.global.mcpServers)).toHaveLength(0)
	})

	test("converts global MCP servers", () => {
		const scan = emptyCursorScan()
		scan.global.mcpJson = {
			mcpServers: {
				MongoDB: {
					command: "npx",
					args: ["-y", "mongodb-mcp-server", "--connectionString", "mongodb://localhost"],
				},
				Figma: {
					url: "http://127.0.0.1:3845/mcp",
				},
			},
		}

		const result = cursorToCanonical(scan)

		expect(result.global.mcpServers.MongoDB).toEqual({
			type: "local",
			command: "npx",
			args: ["-y", "mongodb-mcp-server", "--connectionString", "mongodb://localhost"],
			env: undefined,
		})
		expect(result.global.mcpServers.Figma).toEqual({
			type: "remote",
			url: "http://127.0.0.1:3845/mcp",
			headers: undefined,
		})
	})

	test("converts remote MCP server with OAuth", () => {
		const scan = emptyCursorScan()
		scan.global.mcpJson = {
			mcpServers: {
				"oauth-server": {
					url: "https://api.example.com/mcp",
					auth: {
						CLIENT_ID: "my-client-id",
						CLIENT_SECRET: "my-secret",
						scopes: ["read", "write"],
					},
				},
			},
		}

		const result = cursorToCanonical(scan)

		expect(result.global.mcpServers["oauth-server"].type).toBe("remote")
		expect(result.global.mcpServers["oauth-server"].url).toBe("https://api.example.com/mcp")
		expect(result.global.mcpServers["oauth-server"].oauth).toBeDefined()
	})

	test("converts CLI config permissions", () => {
		const scan = emptyCursorScan()
		scan.global.cliConfig = {
			permissions: {
				allow: ["Shell(ls)", "Shell(git *)"],
				deny: [],
			},
		}

		const result = cursorToCanonical(scan)

		expect(result.global.permissions).toBeDefined()
		expect(result.global.permissions?.bash).toBeDefined()
	})

	test("converts project rules with always-apply", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [
				{
					path: "/test/project/.cursor/rules/always.mdc",
					name: "always",
					content: "---\nalwaysApply: true\n---\n\nAlways apply this rule.",
					frontmatter: { alwaysApply: true },
					body: "Always apply this rule.",
				},
			],
			agents: [],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects).toHaveLength(1)
		expect(result.projects[0].rules).toHaveLength(1)
		expect(result.projects[0].rules[0].alwaysApply).toBe(true)
		expect(result.projects[0].rules[0].ruleType).toBe("always")
	})

	test("converts project rules with globs (file-scoped)", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [
				{
					path: "/test/project/.cursor/rules/api.mdc",
					name: "api",
					content: '---\nglobs: "api/src/**/*.ts"\nalwaysApply: false\n---\n\nAPI rules.',
					frontmatter: { globs: "api/src/**/*.ts", alwaysApply: false },
					body: "API rules.",
				},
			],
			agents: [],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].rules[0].globs).toBe("api/src/**/*.ts")
		expect(result.projects[0].rules[0].ruleType).toBe("file-scoped")
	})

	test("converts project rules with description (intelligent)", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [
				{
					path: "/test/project/.cursor/rules/smart.mdc",
					name: "smart",
					content:
						"---\ndescription: Database migration rules\nalwaysApply: false\n---\n\nSmart rules.",
					frontmatter: { description: "Database migration rules", alwaysApply: false },
					body: "Smart rules.",
				},
			],
			agents: [],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].rules[0].description).toBe("Database migration rules")
		expect(result.projects[0].rules[0].ruleType).toBe("intelligent")
	})

	test("converts project rules with no metadata (manual)", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [
				{
					path: "/test/project/.cursor/rules/manual.mdc",
					name: "manual",
					content: "---\nalwaysApply: false\n---\n\nManual rule content.",
					frontmatter: { alwaysApply: false },
					body: "Manual rule content.",
				},
			],
			agents: [],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].rules[0].ruleType).toBe("manual")
	})

	test("converts project agents", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [],
			agents: [
				{
					path: "/test/project/.cursor/agents/helper.md",
					name: "helper",
					content: "---\nname: helper\ndescription: Helper agent\n---\n\nYou are a helper.",
					frontmatter: { name: "helper", description: "Helper agent" },
					body: "You are a helper.",
				},
			],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].agents).toHaveLength(1)
		expect(result.projects[0].agents[0].name).toBe("helper")
		expect(result.projects[0].agents[0].description).toBe("Helper agent")
	})

	test("converts project commands (plain markdown)", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [],
			agents: [],
			commands: [
				{
					path: "/test/project/.cursor/commands/review.md",
					name: "review",
					content: "Review the code and provide feedback.",
					body: "Review the code and provide feedback.",
				},
			],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].commands).toHaveLength(1)
		expect(result.projects[0].commands[0].name).toBe("review")
		expect(result.projects[0].commands[0].body).toBe("Review the code and provide feedback.")
	})

	test("handles AGENTS.md in project", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [],
			agents: [],
			commands: [],
			skills: [],
			agentsMd: "# Project Instructions\n\nUse TypeScript.",
			agentsMdPath: "/test/project/AGENTS.md",
		})

		const result = cursorToCanonical(scan)

		const agentsMdRule = result.projects[0].rules.find((r) => r.name === "AGENTS.md")
		expect(agentsMdRule).toBeDefined()
		expect(agentsMdRule?.alwaysApply).toBe(true)
	})

	test("handles .cursorrules legacy file", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [],
			agents: [],
			commands: [],
			skills: [],
			cursorRules: "Always use TypeScript strict mode.",
			cursorRulesPath: "/test/project/.cursorrules",
		})

		const result = cursorToCanonical(scan)

		const legacyRule = result.projects[0].rules.find((r) => r.name === ".cursorrules")
		expect(legacyRule).toBeDefined()
		expect(legacyRule?.alwaysApply).toBe(true)
	})

	test("converts glob array to comma-separated string", () => {
		const scan = emptyCursorScan()
		scan.projects.push({
			path: "/test/project",
			rules: [
				{
					path: "/test/project/.cursor/rules/multi.mdc",
					name: "multi",
					content: "---\nglobs:\n  - '*.ts'\n  - '*.tsx'\nalwaysApply: false\n---\n\nBody.",
					frontmatter: { globs: ["*.ts", "*.tsx"], alwaysApply: false },
					body: "Body.",
				},
			],
			agents: [],
			commands: [],
			skills: [],
		})

		const result = cursorToCanonical(scan)

		expect(result.projects[0].rules[0].globs).toBe("*.ts,*.tsx")
	})

	test("converts skills", () => {
		const scan = emptyCursorScan()
		scan.global.skills = [
			{
				path: "/home/.cursor/skills/pdf/SKILL.md",
				name: "pdf",
				description: "PDF manipulation",
				isSymlink: true,
				symlinkTarget: "../../.agents/skills/pdf",
			},
		]

		const result = cursorToCanonical(scan)

		expect(result.global.skills).toHaveLength(1)
		expect(result.global.skills[0].name).toBe("pdf")
		expect(result.global.skills[0].isSymlink).toBe(true)
	})
})
