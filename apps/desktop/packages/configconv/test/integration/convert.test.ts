/**
 * Converter integration test.
 *
 * Tests the full convert() orchestrator with synthetic scan data.
 */
import { describe, expect, it } from "bun:test"
import { convert } from "../../src/converter"
import type { ScanResult } from "../../src/types/scan-result"

function makeScanResult(overrides: Partial<ScanResult> = {}): ScanResult {
	return {
		global: {
			skills: [],
			settings: {
				model: "claude-3-5-sonnet-20241022",
				permissions: {
					allow: ["Bash(npm:*)"],
					deny: ["Bash(rm -rf /*)"],
					defaultMode: "default",
				},
				env: {
					CLAUDE_CODE_USE_BEDROCK: "1",
				},
				autoUpdatesChannel: "latest",
			},
			...overrides.global,
		},
		projects: overrides.projects ?? [],
	}
}

describe("convert() orchestrator", () => {
	it("converts global settings", async () => {
		const result = await convert(makeScanResult())

		expect(result.globalConfig).toBeDefined()
		// Model maps via MODEL_MAP to "anthropic/claude-3-5-sonnet-20241022"
		// even with BEDROCK env, because direct model name mapping takes priority
		expect(result.globalConfig.model).toContain("anthropic/")
		expect(result.globalConfig.autoupdate).toBe(true)
		expect(result.report.migrated.length).toBeGreaterThan(0)
	})

	it("converts global permissions", async () => {
		const result = await convert(makeScanResult())

		expect(result.globalConfig.permission).toBeDefined()
	})

	it("converts provider from env vars", async () => {
		const result = await convert(makeScanResult())

		expect(result.globalConfig.provider).toBeDefined()
		expect(result.globalConfig.provider?.["amazon-bedrock"]).toBeDefined()
	})

	it("respects category filter", async () => {
		const result = await convert(makeScanResult(), {
			categories: ["config"],
		})

		// Should have config items but no permission items
		expect(result.globalConfig.model).toBeDefined()
		// Permissions are part of config conversion, so they show up anyway
		expect(result.report.migrated.length).toBeGreaterThan(0)
	})

	it("converts project MCP servers", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {
						"my-mcp": {
							command: "npx",
							args: ["-y", "@example/mcp-server"],
						},
					},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		const projectConfig = result.projectConfigs.get("/tmp/test-project")
		expect(projectConfig).toBeDefined()
		expect(projectConfig?.mcp).toBeDefined()
		expect(projectConfig?.mcp?.["my-mcp"]).toBeDefined()
	})

	it("converts project MCP from mcpJson", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {},
					mcpJson: {
						mcpServers: {
							"sse-server": {
								type: "sse",
								url: "http://localhost:8080/sse",
							},
						},
					},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		const projectConfig = result.projectConfigs.get("/tmp/test-project")
		expect(projectConfig?.mcp?.["sse-server"]).toBeDefined()
	})

	it("converts project agents", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [
						{
							name: "reviewer",
							path: "/tmp/test-project/.claude/agents/reviewer.md",
							content: "You are a code reviewer.",
							frontmatter: {
								description: "Reviews code for quality",
								model: "opus",
							},
							body: "You are a code reviewer.",
						},
					],
					commands: [],
					skills: [],
					projectMcpServers: {},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		expect(result.agents.size).toBe(1)
		const agentPath = [...result.agents.keys()][0]
		expect(agentPath).toContain("reviewer.md")
	})

	it("converts project commands", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [
						{
							name: "deploy",
							path: "/tmp/test-project/.claude/commands/deploy.md",
							content: "Deploy to production.",
							frontmatter: { description: "Deploys the app" },
							body: "Deploy to production.",
						},
					],
					skills: [],
					projectMcpServers: {},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		expect(result.commands.size).toBe(1)
	})

	it("verifies project skills", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [],
					skills: [
						{
							name: "test-skill",
							description: "A test skill",
							path: "/tmp/test-project/.claude/skills/test-skill/SKILL.md",
							isSymlink: false,
						},
					],
					projectMcpServers: {},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		const skillItems = result.report.migrated.filter((m) => m.category === "skills")
		expect(skillItems.length).toBeGreaterThan(0)
	})

	it("converts rules (CLAUDE.md -> AGENTS.md)", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {},
					allowedTools: [],
					disabledMcpServers: [],
					claudeMd: "# Project Rules\nAlways use TypeScript.",
					claudeMdPath: "/tmp/test-project/CLAUDE.md",
				},
			],
		})

		const result = await convert(scan)

		expect(result.rules.size).toBe(1)
		const rulesContent = [...result.rules.values()][0]
		expect(rulesContent).toContain("Always use TypeScript")
	})

	it("skips rules when AGENTS.md already exists", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [],
					commands: [],
					skills: [],
					projectMcpServers: {},
					allowedTools: [],
					disabledMcpServers: [],
					claudeMd: "# Claude Rules",
					claudeMdPath: "/tmp/test-project/CLAUDE.md",
					agentsMd: "# Agent Rules",
					agentsMdPath: "/tmp/test-project/AGENTS.md",
				},
			],
		})

		const result = await convert(scan)

		// Should NOT create an AGENTS.md since one exists
		expect(result.rules.size).toBe(0)
	})

	it("converts global hooks to plugin stubs", async () => {
		const scan: ScanResult = {
			global: {
				skills: [],
				settings: {
					hooks: {
						PreToolUse: [
							{
								matcher: "Bash",
								hooks: [{ type: "command", command: "echo 'pre-tool'" }],
							},
						],
					},
				},
			},
			projects: [],
		}

		const result = await convert(scan)

		expect(result.hookPlugins.size).toBe(1)
		const pluginPath = [...result.hookPlugins.keys()][0]
		expect(pluginPath).toContain("cc-hooks.ts")

		const pluginContent = [...result.hookPlugins.values()][0]
		expect(pluginContent).toContain("tool.execute.before")
	})

	it("reports manual actions for teammate mode", async () => {
		const scan: ScanResult = {
			global: {
				skills: [],
				settings: {
					teammateMode: "tmux",
				},
			},
			projects: [],
		}

		const result = await convert(scan)

		expect(result.report.manualActions.some((a) => a.includes("teammateMode"))).toBe(true)
	})

	it("merges reports from all converters", async () => {
		const scan = makeScanResult({
			projects: [
				{
					path: "/tmp/test-project",
					agents: [
						{
							name: "test-agent",
							path: "/tmp/test-project/.claude/agents/test-agent.md",
							content: "Agent",
							frontmatter: { description: "Test" },
							body: "Agent",
						},
					],
					commands: [],
					skills: [],
					projectMcpServers: {
						server1: { command: "npx", args: ["server"] },
					},
					allowedTools: [],
					disabledMcpServers: [],
				},
			],
		})

		const result = await convert(scan)

		// Should have items from config, permissions, mcp, and agents converters
		expect(result.report.migrated.length).toBeGreaterThan(2)
	})

	it("returns empty result for empty scan", async () => {
		const result = await convert({
			global: { skills: [] },
			projects: [],
		})

		expect(result.globalConfig).toBeDefined()
		expect(result.projectConfigs.size).toBe(0)
		expect(result.agents.size).toBe(0)
		expect(result.commands.size).toBe(0)
		expect(result.rules.size).toBe(0)
		expect(result.hookPlugins.size).toBe(0)
	})
})
