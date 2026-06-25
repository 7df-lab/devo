/**
 * Tests for the universal converter (cross-format conversions).
 */
import { describe, expect, test } from "bun:test"
import {
	formatName,
	fromCanonical,
	getSupportedConversions,
	toCanonical,
	universalConvert,
} from "../../src/converter/universal"
import type { CursorScanResult } from "../../src/types/cursor"
import type { ScanResult } from "../../src/types/scan-result"

describe("universalConvert", () => {
	test("Claude Code -> Cursor: converts MCP servers", () => {
		const ccScan: ScanResult = {
			global: {
				settings: { model: "claude-opus-4-6" },
				skills: [],
			},
			projects: [
				{
					path: "/test/project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {
						TestServer: {
							command: "npx",
							args: ["-y", "test-mcp"],
							env: { API_KEY: "test" },
						},
					},
				},
			],
		}

		const result = universalConvert({ format: "claude-code", data: ccScan }, { to: "cursor" })

		expect(result.sourceFormat).toBe("claude-code")
		expect(result.targetFormat).toBe("cursor")

		// Project MCP should be converted
		expect(result.projectConfigs.size).toBe(1)
		const projectConfig = result.projectConfigs.get("/test/project") as {
			mcpServers: Record<string, { command: string; args: string[]; env: Record<string, string> }>
		}
		expect(projectConfig.mcpServers.TestServer.command).toBe("npx")
	})

	test("Claude Code -> Cursor: converts CLAUDE.md to .cursor/rules/*.mdc", () => {
		const ccScan: ScanResult = {
			global: { skills: [] },
			projects: [
				{
					path: "/test/project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {},
					claudeMd: "# Project Instructions\n\nUse TypeScript.",
					claudeMdPath: "/test/project/CLAUDE.md",
				},
			],
		}

		const result = universalConvert({ format: "claude-code", data: ccScan }, { to: "cursor" })

		expect(result.rules.size).toBeGreaterThan(0)
		const ruleEntries = [...result.rules.entries()]
		const hasProjectRule = ruleEntries.some(([path]) =>
			path.includes("/test/project/.cursor/rules/"),
		)
		expect(hasProjectRule).toBe(true)
	})

	test("Claude Code -> Cursor: converts agents", () => {
		const ccScan: ScanResult = {
			global: { skills: [] },
			projects: [
				{
					path: "/test/project",
					agents: [
						{
							path: "/test/project/.claude/agents/review.md",
							name: "review",
							content: "---\nname: review\ndescription: Code reviewer\n---\n\nReview the code.",
							frontmatter: { name: "review", description: "Code reviewer" },
							body: "Review the code.",
						},
					],
					commands: [],
					skills: [],
					projectMcpServers: {},
				},
			],
		}

		const result = universalConvert({ format: "claude-code", data: ccScan }, { to: "cursor" })

		expect(result.agents.size).toBe(1)
		const agentEntries = [...result.agents.entries()]
		expect(agentEntries[0][0]).toContain(".cursor/agents/review.md")
	})

	test("Cursor -> Devo: converts MCP servers", () => {
		const cursorScan: CursorScanResult = {
			global: {
				skills: [],
				commands: [],
				agents: [],
				mcpJson: {
					mcpServers: {
						Figma: { url: "http://127.0.0.1:3845/mcp" },
					},
				},
			},
			projects: [],
		}

		const result = universalConvert({ format: "cursor", data: cursorScan }, { to: "devo" })

		expect(result.targetFormat).toBe("devo")
		const config = result.globalConfig as Record<string, unknown>
		const mcp = config.mcp as Record<string, { type: string; url: string }>
		expect(mcp.Figma.type).toBe("remote")
		expect(mcp.Figma.url).toBe("http://127.0.0.1:3845/mcp")
	})

	test("Cursor -> Devo: converts rules to AGENTS.md", () => {
		const cursorScan: CursorScanResult = {
			global: { skills: [], commands: [], agents: [] },
			projects: [
				{
					path: "/test/project",
					rules: [
						{
							path: "/test/project/.cursor/rules/always.mdc",
							name: "always",
							content: "---\nalwaysApply: true\n---\n\nAlways apply this.",
							frontmatter: { alwaysApply: true },
							body: "Always apply this.",
						},
					],
					agents: [],
					commands: [],
					skills: [],
				},
			],
		}

		const result = universalConvert({ format: "cursor", data: cursorScan }, { to: "devo" })

		expect(result.rules.size).toBeGreaterThan(0)
		const rulesEntries = [...result.rules.entries()]
		const agentsMd = rulesEntries.find(([path]) => path.endsWith("AGENTS.md"))
		expect(agentsMd).toBeDefined()
		expect(agentsMd?.[1]).toContain("Always apply this")
	})

	test("Cursor -> Claude Code: converts rules to CLAUDE.md", () => {
		const cursorScan: CursorScanResult = {
			global: { skills: [], commands: [], agents: [] },
			projects: [
				{
					path: "/test/project",
					rules: [
						{
							path: "/test/project/.cursor/rules/main.mdc",
							name: "main",
							content: "---\nalwaysApply: true\n---\n\nUse tabs for indentation.",
							frontmatter: { alwaysApply: true },
							body: "Use tabs for indentation.",
						},
					],
					agents: [],
					commands: [],
					skills: [],
				},
			],
		}

		const result = universalConvert({ format: "cursor", data: cursorScan }, { to: "claude-code" })

		expect(result.rules.size).toBeGreaterThan(0)
		const rulesEntries = [...result.rules.entries()]
		const claudeMd = rulesEntries.find(([path]) => path.endsWith("CLAUDE.md"))
		expect(claudeMd).toBeDefined()
		expect(claudeMd?.[1]).toContain("Use tabs for indentation")
	})

	test("Cursor -> Claude Code: converts file-scoped rules to .claude/rules/", () => {
		const cursorScan: CursorScanResult = {
			global: { skills: [], commands: [], agents: [] },
			projects: [
				{
					path: "/test/project",
					rules: [
						{
							path: "/test/project/.cursor/rules/api.mdc",
							name: "api",
							content: '---\nglobs: "api/**/*.ts"\nalwaysApply: false\n---\n\nAPI-specific rules.',
							frontmatter: { globs: "api/**/*.ts", alwaysApply: false },
							body: "API-specific rules.",
						},
					],
					agents: [],
					commands: [],
					skills: [],
				},
			],
		}

		const result = universalConvert({ format: "cursor", data: cursorScan }, { to: "claude-code" })

		// File-scoped rules should generate manual action warnings
		expect(result.report.manualActions.length).toBeGreaterThan(0)
		// And should be written to .claude/rules/
		const ruleEntries = [...result.rules.entries()]
		const claudeRule = ruleEntries.find(([path]) => path.includes(".claude/rules/"))
		expect(claudeRule).toBeDefined()
	})
})

describe("toCanonical", () => {
	test("tags source format correctly", () => {
		const ccScan: ScanResult = {
			global: { skills: [] },
			projects: [],
		}

		const result = toCanonical({ format: "claude-code", data: ccScan })
		expect(result.sourceFormat).toBe("claude-code")
	})

	test("preserves MCP server details through canonical form", () => {
		const ccScan: ScanResult = {
			global: { skills: [] },
			projects: [
				{
					path: "/test",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {
						RemoteServer: {
							type: "sse",
							url: "https://example.com/sse",
							headers: { Authorization: "Bearer token" },
						},
					},
				},
			],
		}

		const canonical = toCanonical({ format: "claude-code", data: ccScan })

		expect(canonical.projects[0].mcpServers.RemoteServer).toEqual({
			type: "remote",
			url: "https://example.com/sse",
			headers: { Authorization: "Bearer token" },
		})
	})
})

describe("fromCanonical", () => {
	test("produces correct target format tag", () => {
		const canonical = toCanonical({
			format: "claude-code",
			data: { global: { skills: [] }, projects: [] },
		})

		const result = fromCanonical(canonical, "cursor")
		expect(result.targetFormat).toBe("cursor")
	})
})

describe("getSupportedConversions", () => {
	test("returns 6 conversion pairs", () => {
		const pairs = getSupportedConversions()

		expect(pairs).toHaveLength(6)
		expect(pairs).toContainEqual({ from: "claude-code", to: "devo" })
		expect(pairs).toContainEqual({ from: "claude-code", to: "cursor" })
		expect(pairs).toContainEqual({ from: "devo", to: "claude-code" })
		expect(pairs).toContainEqual({ from: "devo", to: "cursor" })
		expect(pairs).toContainEqual({ from: "cursor", to: "claude-code" })
		expect(pairs).toContainEqual({ from: "cursor", to: "devo" })
	})
})

describe("formatName", () => {
	test("returns human-readable names", () => {
		expect(formatName("claude-code")).toBe("Claude Code")
		expect(formatName("devo")).toBe("Devo")
		expect(formatName("cursor")).toBe("Cursor")
	})
})
