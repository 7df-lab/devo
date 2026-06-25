/**
 * Differ tests.
 *
 * Tests for comparing Claude Code configs against Devo configs.
 */
import { describe, expect, it } from "bun:test"
import { diff } from "../../src/differ"
import type { ScanResult } from "../../src/types/scan-result"

describe("differ", () => {
	describe("diff() with empty scan result", () => {
		it("returns empty diff for empty scan result", async () => {
			const emptyResult: ScanResult = {
				global: { skills: [] },
				projects: [],
			}

			const result = await diff(emptyResult)

			expect(result.onlyInClaudeCode).toEqual([])
			expect(result.onlyInDevo).toEqual([])
			expect(result.different).toEqual([])
			expect(result.matching).toEqual([])
			expect(result.report).toBeDefined()
		})
	})

	describe("diff() with model", () => {
		it("detects model in CC but not in OC", async () => {
			const scanResult: ScanResult = {
				global: {
					skills: [],
					settings: {
						model: "claude-3-sonnet",
					},
				},
				projects: [],
			}

			const result = await diff(scanResult)

			// Depending on whether OC config exists, this will be in different or onlyInCC
			const hasModelDiff =
				result.onlyInClaudeCode.some(
					(item) => item.category === "config" && item.key === "model",
				) || result.different.some((item) => item.category === "config" && item.key === "model")

			expect(hasModelDiff).toBe(true)
		})
	})

	describe("diff() with MCP servers", () => {
		it("detects MCP servers in CC project", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project",
						agents: [],
						commands: [],
						skills: [],
						projectMcpServers: {
							"test-mcp": {
								command: "npx",
								args: ["-y", "test-mcp-server"],
							},
						},
						allowedTools: [],
						disabledMcpServers: [],
					},
				],
			}

			const result = await diff(scanResult)

			// MCP server should be detected as only in CC (since project doesn't exist)
			const hasMcpItem = result.onlyInClaudeCode.some(
				(item) => item.category === "mcp" && item.key === "test-mcp",
			)

			expect(hasMcpItem).toBe(true)
		})

		it("detects MCP from mcpJson", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project-2",
						agents: [],
						commands: [],
						skills: [],
						projectMcpServers: {},
						mcpJson: {
							mcpServers: {
								"json-mcp": {
									type: "sse",
									url: "http://localhost:3000/sse",
								},
							},
						},
						allowedTools: [],
						disabledMcpServers: [],
					},
				],
			}

			const result = await diff(scanResult)

			const hasMcpItem = result.onlyInClaudeCode.some(
				(item) => item.category === "mcp" && item.key === "json-mcp",
			)

			expect(hasMcpItem).toBe(true)
		})
	})

	describe("diff() with agents", () => {
		it("detects agents in CC project", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project-3",
						agents: [
							{
								name: "code-reviewer",
								path: "/tmp/test-project-3/.claude/agents/code-reviewer.md",
								content: "# Code Reviewer\nReviews code",
								frontmatter: {},
								body: "# Code Reviewer\nReviews code",
							},
						],
						commands: [],
						skills: [],
						projectMcpServers: {},
						allowedTools: [],
						disabledMcpServers: [],
					},
				],
			}

			const result = await diff(scanResult)

			const hasAgentItem = result.onlyInClaudeCode.some(
				(item) => item.category === "agents" && item.key === "code-reviewer",
			)

			expect(hasAgentItem).toBe(true)
		})
	})

	describe("diff() with commands", () => {
		it("detects commands in CC project", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project-4",
						agents: [],
						commands: [
							{
								name: "deploy",
								path: "/tmp/test-project-4/.claude/commands/deploy.md",
								content: "# Deploy\nDeploys the app",
								frontmatter: {},
								body: "# Deploy\nDeploys the app",
							},
						],
						skills: [],
						projectMcpServers: {},
						allowedTools: [],
						disabledMcpServers: [],
					},
				],
			}

			const result = await diff(scanResult)

			const hasCommandItem = result.onlyInClaudeCode.some(
				(item) => item.category === "commands" && item.key === "deploy",
			)

			expect(hasCommandItem).toBe(true)
		})
	})

	describe("diff() with permissions", () => {
		it("detects permissions in CC", async () => {
			const scanResult: ScanResult = {
				global: {
					skills: [],
					settings: {
						permissions: {
							allow: ["Bash(npm:*)"],
							deny: [],
							defaultMode: "default",
						},
					},
				},
				projects: [],
			}

			const result = await diff(scanResult)

			// Permissions should be in onlyInCC or different
			const hasPermissionsDiff =
				result.onlyInClaudeCode.some((item) => item.category === "permissions") ||
				result.different.some((item) => item.category === "permissions")

			expect(hasPermissionsDiff).toBe(true)
		})
	})

	describe("diff() with rules", () => {
		it("detects CLAUDE.md without AGENTS.md", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project-5",
						agents: [],
						commands: [],
						skills: [],
						projectMcpServers: {},
						allowedTools: [],
						disabledMcpServers: [],
						claudeMd: "# Project Rules\nSome rules here",
						claudeMdPath: "/tmp/test-project-5/CLAUDE.md",
						// agentsMd is undefined
					},
				],
			}

			const result = await diff(scanResult)

			const hasRulesItem = result.onlyInClaudeCode.some(
				(item) => item.category === "rules" && item.key === "CLAUDE.md",
			)

			expect(hasRulesItem).toBe(true)
		})

		it("marks rules as matching when both exist", async () => {
			const scanResult: ScanResult = {
				global: { skills: [] },
				projects: [
					{
						path: "/tmp/test-project-6",
						agents: [],
						commands: [],
						skills: [],
						projectMcpServers: {},
						allowedTools: [],
						disabledMcpServers: [],
						claudeMd: "# Claude Rules",
						claudeMdPath: "/tmp/test-project-6/CLAUDE.md",
						agentsMd: "# Agents Rules",
						agentsMdPath: "/tmp/test-project-6/AGENTS.md",
					},
				],
			}

			const result = await diff(scanResult)

			const hasMatchingRules = result.matching.some((item) => item.category === "rules")

			expect(hasMatchingRules).toBe(true)
		})
	})

	describe("diff() report", () => {
		it("generates appropriate report for differences", async () => {
			const scanResult: ScanResult = {
				global: {
					skills: [],
					settings: {
						model: "claude-3-opus",
						permissions: {
							defaultMode: "bypassPermissions",
						},
					},
				},
				projects: [
					{
						path: "/tmp/test-project-7",
						agents: [
							{
								name: "helper",
								path: "/tmp/test-project-7/.claude/agents/helper.md",
								content: "Helper agent",
								frontmatter: {},
								body: "Helper agent",
							},
						],
						commands: [],
						skills: [],
						projectMcpServers: {},
						allowedTools: [],
						disabledMcpServers: [],
					},
				],
			}

			const result = await diff(scanResult)

			expect(result.report).toBeDefined()
			// Should have some items to report
			const totalItems =
				result.onlyInClaudeCode.length +
				result.onlyInDevo.length +
				result.different.length +
				result.matching.length

			expect(totalItems).toBeGreaterThan(0)
		})
	})
})
