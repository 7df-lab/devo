import { describe, expect, test } from "bun:test"
import {
	buildClaudeCodeProviderMigrationPreview,
	executeClaudeCodeProviderMigration,
} from "./claude-code-provider-migration"

function scanResultWithEnv(env?: Record<string, string>) {
	return {
		format: "claude-code",
		data: {
			global: {
				settings: env ? { env } : undefined,
				skills: [],
			},
			projects: [],
		},
	}
}

const exampleEnv = {
	ANTHROPIC_BASE_URL: "https://api.deepseek.com/anthropic",
	ANTHROPIC_AUTH_TOKEN: "sk-xxxxxxxx",
	ANTHROPIC_MODEL: "deepseek-v4-pro[1m]",
	ANTHROPIC_DEFAULT_HAIKU_MODEL: "deepseek-v4-flash",
	ANTHROPIC_DEFAULT_SONNET_MODEL: "deepseek-v4-flash",
	ANTHROPIC_DEFAULT_OPUS_MODEL: "deepseek-v4-pro[1m]",
	ENABLE_TOOL_SEARCH: "true",
}

describe("Claude Code provider migration", () => {
	test("builds a sanitized provider preview without exposing the API key", () => {
		const result = buildClaudeCodeProviderMigrationPreview(scanResultWithEnv(exampleEnv))

		expect(result.category?.category).toBe("config")
		expect(result.category?.files[0].path).toBe("provider/upsert:claude-code")
		expect(result.category?.files[0].content).toContain("Provider ID: claude-code")
		expect(result.category?.files[0].content).toContain(
			"Base URL: https://api.deepseek.com/anthropic",
		)
		expect(result.category?.files[0].content).toContain("Wire API: anthropic_messages")
		expect(result.category?.files[0].content).toContain("Default model: deepseek-v4-pro[1m]")
		expect(result.category?.files[0].content).toContain("API key: detected")
		expect(result.category?.files[0].content).not.toContain("sk-xxxxxxxx")
	})

	test("executes provider upserts for each unique imported model", async () => {
		const calls: Array<{ method: string; params: Record<string, unknown> }> = []

		const result = await executeClaudeCodeProviderMigration(
			scanResultWithEnv(exampleEnv),
			async (method, params) => {
				calls.push({ method, params: params as Record<string, unknown> })
				return {}
			},
		)

		expect(result.errors).toEqual([])
		expect(result.filesWritten).toEqual([
			"provider/upsert:claude-code/deepseek-v4-pro-1m-claude-code",
			"provider/upsert:claude-code/deepseek-v4-flash-claude-code",
		])
		expect(calls).toHaveLength(2)
		expect(calls[0].method).toBe("provider/upsert")
		expect(calls[0].params).toEqual({
			provider_vendor: {
				name: "claude-code",
				base_url: "https://api.deepseek.com/anthropic",
				credential: null,
				headers: null,
				wire_apis: ["anthropic_messages"],
				enabled: true,
			},
			model_binding: {
				binding_id: "deepseek-v4-pro-1m-claude-code",
				model_slug: "deepseek-v4-pro[1m]",
				provider: "claude-code",
				request_model: "deepseek-v4-pro[1m]",
				display_name: "deepseek-v4-pro[1m]",
				invocation_method: "anthropic_messages",
				default_reasoning_effort: null,
				enabled: true,
			},
			default_model_binding: "deepseek-v4-pro-1m-claude-code",
			api_key: "sk-xxxxxxxx",
		})
		expect(calls[1].params).toMatchObject({
			default_model_binding: undefined,
			api_key: "sk-xxxxxxxx",
		})
	})

	test("reports missing settings without throwing or calling provider upsert", async () => {
		const calls: unknown[] = []
		const preview = buildClaudeCodeProviderMigrationPreview(scanResultWithEnv())
		const result = await executeClaudeCodeProviderMigration(scanResultWithEnv(), async () => {
			calls.push(true)
			return {}
		})

		expect(preview.category).toBeNull()
		expect(preview.warnings.length + preview.manualActions.length).toBeGreaterThan(0)
		expect(result.errors).toEqual([])
		expect(result.warnings.length + result.manualActions.length).toBeGreaterThan(0)
		expect(calls).toEqual([])
	})

	test("imports model bindings without an API key and reports manual action", async () => {
		const calls: Array<Record<string, unknown>> = []

		const result = await executeClaudeCodeProviderMigration(
			scanResultWithEnv({
				ANTHROPIC_BASE_URL: "https://api.deepseek.com/anthropic",
				ANTHROPIC_MODEL: "deepseek-v4-pro[1m]",
			}),
			async (_method, params) => {
				calls.push(params as Record<string, unknown>)
				return {}
			},
		)

		expect(result.errors).toEqual([])
		expect(result.manualActions.some((item) => item.includes("ANTHROPIC_AUTH_TOKEN"))).toBe(true)
		expect(calls).toHaveLength(1)
		expect(calls[0].api_key).toBeUndefined()
	})
})
