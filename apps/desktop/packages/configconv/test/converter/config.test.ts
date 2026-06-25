import { describe, expect, test } from "bun:test"
import { convertConfig } from "../../src/converter/config"
import type { ClaudeSettings } from "../../src/types/claude-code"

describe("convertConfig", () => {
	test("returns empty config when no settings", () => {
		const { config, report } = convertConfig({})
		expect(config.$schema).toBe("https://devo.ai/config.json")
		expect(report.skipped.length).toBe(1)
	})

	test("converts model", () => {
		const settings: ClaudeSettings = { model: "claude-opus-4-6" }
		const { config } = convertConfig({ settings })
		expect(config.model).toBe("anthropic/claude-opus-4-6")
		expect(config.small_model).toBe("anthropic/claude-sonnet-4-5")
	})

	test("uses defaultModel when no model in settings", () => {
		const { config } = convertConfig({
			settings: {},
			defaultModel: "anthropic/claude-sonnet-4-5",
		})
		expect(config.model).toBe("anthropic/claude-sonnet-4-5")
	})

	test("uses custom defaultSmallModel", () => {
		const { config } = convertConfig({
			settings: { model: "claude-opus-4-6" },
			defaultSmallModel: "anthropic/claude-3-5-haiku-20241022",
		})
		expect(config.small_model).toBe("anthropic/claude-3-5-haiku-20241022")
	})

	test("converts Bedrock provider", () => {
		const settings: ClaudeSettings = {
			env: { CLAUDE_CODE_USE_BEDROCK: "1" },
		}
		const { config, report } = convertConfig({ settings })
		expect(config.provider).toBeDefined()
		expect(config.provider?.["amazon-bedrock"]).toBeDefined()
		expect(report.migrated.some((m) => m.target.includes("amazon-bedrock"))).toBe(true)
	})

	test("converts Vertex provider", () => {
		const settings: ClaudeSettings = {
			env: { CLAUDE_CODE_USE_VERTEX: "1" },
		}
		const { config } = convertConfig({ settings })
		expect(config.provider?.["google-vertex"]).toBeDefined()
	})

	test("warns about AWS credentials", () => {
		const settings: ClaudeSettings = {
			env: {
				CLAUDE_CODE_USE_BEDROCK: "1",
				AWS_ACCESS_KEY_ID: "AKIA...",
				AWS_SECRET_ACCESS_KEY: "secret",
			},
		}
		const { report } = convertConfig({ settings })
		expect(report.manualActions.length).toBeGreaterThan(0)
		expect(report.manualActions.some((a) => a.includes("AWS credentials"))).toBe(true)
	})

	test("references Anthropic API key with env interpolation", () => {
		const settings: ClaudeSettings = {
			env: { ANTHROPIC_API_KEY: "sk-ant-api..." },
		}
		const { config, report } = convertConfig({ settings })
		expect(config.provider?.anthropic?.options?.apiKey).toBe("{env:ANTHROPIC_API_KEY}")
		expect(report.warnings.some((w) => w.includes("ANTHROPIC_API_KEY"))).toBe(true)
	})

	test("converts autoUpdatesChannel to autoupdate", () => {
		const settings: ClaudeSettings = {
			autoUpdatesChannel: "stable",
		}
		const { config } = convertConfig({ settings })
		expect(config.autoupdate).toBe(true)
	})

	test("converts permissions", () => {
		const settings: ClaudeSettings = {
			permissions: {
				allow: ["Read", "Bash(git *)"],
				defaultMode: "default",
			},
		}
		const { config } = convertConfig({ settings })
		expect(config.permission).toBeDefined()
	})

	test("reports manual action for teammate mode", () => {
		const settings: ClaudeSettings = { teammateMode: "enabled" }
		const { report } = convertConfig({ settings })
		expect(report.manualActions.some((a) => a.includes("teammateMode"))).toBe(true)
	})

	test("reports unmapped environment variables", () => {
		const settings: ClaudeSettings = {
			env: { CUSTOM_VAR: "value", ANOTHER: "test" },
		}
		const { report } = convertConfig({ settings })
		expect(report.manualActions.some((a) => a.includes("CUSTOM_VAR"))).toBe(true)
	})

	test("reports manual action for hooks", () => {
		const settings: ClaudeSettings = {
			hooks: {
				PreToolUse: [{ hooks: [{ type: "command", command: "echo test" }] }],
			},
		}
		const { report } = convertConfig({ settings })
		expect(report.manualActions.some((a) => a.includes("hooks"))).toBe(true)
	})
})
