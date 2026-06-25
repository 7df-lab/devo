/**
 * Tests for Canonical -> Cursor conversion.
 */
import { describe, expect, test } from "bun:test"
import { canonicalToCursor } from "../../src/converter/from-canonical/to-cursor"
import type { CanonicalScanResult } from "../../src/types/canonical"

function emptyCanonical(): CanonicalScanResult {
	return {
		sourceFormat: "claude-code",
		global: {
			mcpServers: {},
			skills: [],
			commands: [],
			agents: [],
		},
		projects: [],
	}
}

describe("canonicalToCursor", () => {
	test("converts empty scan result", () => {
		const result = canonicalToCursor(emptyCanonical())

		expect(result.targetFormat).toBe("cursor")
		expect(result.agents.size).toBe(0)
		expect(result.commands.size).toBe(0)
		expect(result.rules.size).toBe(0)
	})

	test("converts global MCP servers to ~/.cursor/mcp.json format", () => {
		const scan = emptyCanonical()
		scan.global.mcpServers = {
			MongoDB: {
				type: "local",
				command: "npx",
				args: ["-y", "mongodb-mcp-server"],
				env: { DB_URI: "mongodb://localhost" },
			},
			Linear: {
				type: "remote",
				url: "https://mcp.linear.app/mcp",
			},
		}

		const result = canonicalToCursor(scan)

		const config = result.globalConfig as { mcpServers: Record<string, unknown> }
		expect(config.mcpServers.MongoDB).toEqual({
			command: "npx",
			args: ["-y", "mongodb-mcp-server"],
			env: { DB_URI: "mongodb://localhost" },
		})
		expect(config.mcpServers.Linear).toEqual({
			url: "https://mcp.linear.app/mcp",
		})
	})

	test("converts remote MCP with headers", () => {
		const scan = emptyCanonical()
		scan.global.mcpServers = {
			github: {
				type: "remote",
				url: "https://api.githubcopilot.com/mcp/",
				// biome-ignore lint/suspicious/noTemplateCurlyInString: intentional literal
				headers: { Authorization: "Bearer ${GITHUB_TOKEN}" },
			},
		}

		const result = canonicalToCursor(scan)

		const config = result.globalConfig as {
			mcpServers: Record<string, { url: string; headers: Record<string, string> }>
		}
		expect(config.mcpServers.github.headers).toEqual({
			// biome-ignore lint/suspicious/noTemplateCurlyInString: intentional literal
			Authorization: "Bearer ${GITHUB_TOKEN}",
		})
	})

	test("generates .mdc rule files from canonical rules", () => {
		const scan = emptyCanonical()
		scan.projects.push({
			path: "/test/project",
			mcpServers: {},
			rules: [
				{
					path: "/test/project/CLAUDE.md",
					name: "CLAUDE.md",
					content: "# Project Rules\n\nUse TypeScript strict mode.",
					alwaysApply: true,
					ruleType: "always",
				},
			],
			skills: [],
			commands: [],
			agents: [],
		})

		const result = canonicalToCursor(scan)

		expect(result.rules.size).toBeGreaterThan(0)
		// Find the generated rule
		const ruleEntries = [...result.rules.entries()]
		expect(ruleEntries.length).toBe(1)

		const [path, content] = ruleEntries[0]
		expect(path).toContain(".cursor/rules/")
		expect(path).toEndWith(".mdc")
		expect(content).toContain("alwaysApply")
	})

	test("generates .mdc rule files with globs for file-scoped rules", () => {
		const scan = emptyCanonical()
		scan.projects.push({
			path: "/test/project",
			mcpServers: {},
			rules: [
				{
					path: "/test/project/.devo/rules/api.md",
					name: "api-rules",
					content: "Use NestJS patterns for the API.",
					globs: "api/src/**/*.ts",
					alwaysApply: false,
					ruleType: "file-scoped",
				},
			],
			skills: [],
			commands: [],
			agents: [],
		})

		const result = canonicalToCursor(scan)

		const ruleEntries = [...result.rules.entries()]
		expect(ruleEntries.length).toBe(1)

		const [, content] = ruleEntries[0]
		expect(content).toContain("globs")
		expect(content).toContain("api/src/**/*.ts")
		expect(content).toContain("alwaysApply")
	})

	test("converts agents to Cursor format (minimal frontmatter)", () => {
		const scan = emptyCanonical()
		scan.projects.push({
			path: "/test/project",
			mcpServers: {},
			rules: [],
			skills: [],
			commands: [],
			agents: [
				{
					path: "/test/project/.devo/agents/build.md",
					name: "build",
					content: "---\ndescription: Build agent\n---\n\nYou are a build agent.",
					frontmatter: { description: "Build agent" },
					body: "You are a build agent.",
					description: "Build agent",
					mode: "primary",
				},
			],
		})

		const result = canonicalToCursor(scan)

		expect(result.agents.size).toBe(1)
		const agentEntries = [...result.agents.entries()]
		const [path, content] = agentEntries[0]
		expect(path).toContain(".cursor/agents/build.md")
		expect(content).toContain("build")
	})

	test("converts commands to plain markdown (no frontmatter)", () => {
		const scan = emptyCanonical()
		scan.projects.push({
			path: "/test/project",
			mcpServers: {},
			rules: [],
			skills: [],
			agents: [],
			commands: [
				{
					path: "/test/project/.devo/commands/commit.md",
					name: "commit",
					content: "---\ndescription: Commit changes\n---\n\nCommit all staged changes.",
					frontmatter: { description: "Commit changes" },
					body: "Commit all staged changes.",
					description: "Commit changes",
				},
			],
		})

		const result = canonicalToCursor(scan)

		expect(result.commands.size).toBe(1)
		const cmdEntries = [...result.commands.entries()]
		const [path, content] = cmdEntries[0]
		expect(path).toContain(".cursor/commands/commit.md")
		// Cursor commands are plain markdown
		expect(content).toBe("Commit all staged changes.")
		expect(content).not.toContain("---")
	})

	test("reports warnings for embedded credentials in URLs", () => {
		const scan = emptyCanonical()
		scan.global.mcpServers = {
			secret: {
				type: "remote",
				url: "https://api.example.com/sse?token=abc123",
			},
		}

		const result = canonicalToCursor(scan)

		expect(result.report.warnings.length).toBeGreaterThan(0)
		expect(result.report.warnings[0]).toContain("credentials")
	})

	test("converts project MCP to .cursor/mcp.json", () => {
		const scan = emptyCanonical()
		scan.projects.push({
			path: "/test/project",
			mcpServers: {
				LocalDB: {
					type: "local",
					command: "node",
					args: ["server.js"],
				},
			},
			rules: [],
			skills: [],
			commands: [],
			agents: [],
		})

		const result = canonicalToCursor(scan)

		expect(result.projectConfigs.size).toBe(1)
		const config = result.projectConfigs.get("/test/project") as {
			mcpServers: Record<string, { command: string }>
		}
		expect(config.mcpServers.LocalDB.command).toBe("node")
	})

	test("tracks all conversions in report", () => {
		const scan = emptyCanonical()
		scan.global.mcpServers = {
			TestServer: { type: "remote", url: "https://test.com/mcp" },
		}
		scan.projects.push({
			path: "/test/project",
			mcpServers: {},
			rules: [
				{
					path: "/test/CLAUDE.md",
					name: "CLAUDE.md",
					content: "Test rules",
					alwaysApply: true,
					ruleType: "always",
				},
			],
			skills: [],
			commands: [
				{
					path: "/test/.devo/commands/test.md",
					name: "test",
					content: "Run tests",
					frontmatter: {},
					body: "Run tests",
				},
			],
			agents: [],
		})

		const result = canonicalToCursor(scan)

		expect(result.report.converted.length).toBeGreaterThan(0)
		const categories = result.report.converted.map((c) => c.category)
		expect(categories).toContain("mcp")
		expect(categories).toContain("rules")
		expect(categories).toContain("commands")
	})
})
