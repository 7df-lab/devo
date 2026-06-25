import { describe, expect, test } from "bun:test"
import type { ConversionResult } from "../../src/types/conversion-result"
import { createEmptyReport } from "../../src/types/report"
import { validate } from "../../src/validator"

function makeConversion(overrides: Partial<ConversionResult> = {}): ConversionResult {
	return {
		globalConfig: {},
		projectConfigs: new Map(),
		agents: new Map(),
		commands: new Map(),
		rules: new Map(),
		hookPlugins: new Map(),
		report: createEmptyReport(),
		...overrides,
	}
}

describe("validate", () => {
	test("passes for empty conversion", () => {
		const result = validate(makeConversion())
		expect(result.valid).toBe(true)
		expect(result.errors.length).toBe(0)
	})

	test("passes for valid config", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					model: "anthropic/claude-opus-4-6",
					small_model: "anthropic/claude-sonnet-4-5",
					autoupdate: true,
				},
			}),
		)
		expect(result.valid).toBe(true)
	})

	test("fails for model without provider prefix", () => {
		const result = validate(
			makeConversion({
				globalConfig: { model: "claude-opus-4-6" },
			}),
		)
		expect(result.valid).toBe(false)
		expect(result.errors.some((e) => e.path === "globalConfig.model")).toBe(true)
	})

	test("fails for small_model without provider prefix", () => {
		const result = validate(
			makeConversion({
				globalConfig: { small_model: "haiku" },
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("validates MCP local server", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					mcp: {
						valid: { type: "local", command: ["node", "server.js"] },
					},
				},
			}),
		)
		expect(result.valid).toBe(true)
	})

	test("fails for MCP local with empty command", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					mcp: {
						// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
						broken: { type: "local", command: [] } as any,
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
		expect(result.errors.some((e) => e.path.includes("command"))).toBe(true)
	})

	test("fails for MCP remote without url", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					mcp: {
						// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
						broken: { type: "remote" } as any,
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
		expect(result.errors.some((e) => e.path.includes("url"))).toBe(true)
	})

	test("fails for MCP with missing type", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					mcp: {
						// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
						broken: { command: ["node"] } as any,
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("warns about embedded credentials in MCP URLs", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					mcp: {
						remote: {
							type: "remote",
							url: "https://example.com?token=secret",
						},
					},
				},
			}),
		)
		expect(result.warnings.some((w) => w.includes("embedded credentials"))).toBe(true)
	})

	test("validates agent temperature range", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					agent: {
						bad: { temperature: 5.0 },
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("validates agent mode", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					agent: {
						// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
						bad: { mode: "invalid" as any },
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("validates agent steps is positive integer", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					agent: {
						bad: { steps: -1 },
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("fails for agent model without provider prefix", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					agent: {
						bad: { model: "opus" },
					},
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("validates permission actions", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
					permission: { "*": "invalid" as any },
				},
			}),
		)
		expect(result.valid).toBe(false)
	})

	test("passes for valid nested permissions", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					permission: {
						"*": "ask",
						bash: { "git *": "allow", "rm -rf *": "deny" },
						read: "allow",
					},
				},
			}),
		)
		expect(result.valid).toBe(true)
	})

	test("warns about hardcoded secrets in provider config", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					provider: {
						anthropic: {
							options: { apiKey: "sk-ant-actual-secret" },
						},
					},
				},
			}),
		)
		expect(result.warnings.some((w) => w.includes("hardcoded secret"))).toBe(true)
	})

	test("no warning for env interpolated secrets", () => {
		const result = validate(
			makeConversion({
				globalConfig: {
					provider: {
						anthropic: {
							options: { apiKey: "{env:ANTHROPIC_API_KEY}" },
						},
					},
				},
			}),
		)
		expect(result.warnings.filter((w) => w.includes("hardcoded secret")).length).toBe(0)
	})

	test("validates agent file frontmatter", () => {
		const agents = new Map<string, string>()
		agents.set("good.md", "---\ndescription: test\n---\nbody")
		agents.set("bad.md", "No frontmatter here")

		const result = validate(makeConversion({ agents }))
		expect(result.errors.some((e) => e.path.includes("bad.md"))).toBe(true)
		expect(result.errors.some((e) => e.path.includes("good.md"))).toBe(false)
	})

	test("validates project configs independently", () => {
		// biome-ignore lint/suspicious/noExplicitAny: testing with intentionally invalid data
		const projectConfigs = new Map<string, any>()
		projectConfigs.set("/project1", { model: "bad-model" })
		projectConfigs.set("/project2", { model: "anthropic/good-model" })

		const result = validate(makeConversion({ projectConfigs }))
		expect(result.errors.some((e) => e.path.includes("/project1"))).toBe(true)
		expect(result.errors.some((e) => e.path.includes("/project2"))).toBe(false)
	})
})
