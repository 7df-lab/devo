import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import type { ProviderCatalogData, ProvidersData } from "../../hooks/use-devo-data"
import type { ModelDirectoryData } from "../../hooks/use-model-directory"
import { buildPickerModels, filterPickerModels } from "./model-picker-data"

const catalog: ProviderCatalogData = {
	providers: [
		{
			id: "openai-codex",
			name: "OpenAI Codex",
			enabled: true,
			wireApis: [],
			models: {
				"gpt-codex": { name: "Coding model" },
				disabled: { enabled: false },
			},
		},
		{
			id: "deepseek",
			name: "DeepSeek",
			enabled: true,
			wireApis: [],
			models: { flash: { name: "Flash" } },
		},
		{
			id: "disabled",
			name: "Disabled provider",
			enabled: false,
			wireApis: [],
			models: { hidden: {} },
		},
	],
	templateIds: new Set(["openai-codex", "deepseek"]),
	connectedIds: new Set(["deepseek"]),
	connectionModels: {},
}
const available = {
	providers: [
		{
			id: "session",
			name: "Session",
			models: { "deepseek/flash": { name: "Flash" } },
		},
	],
	defaults: {},
} as ProvidersData

describe("desktop model directory", () => {
	test("merges ready models with the catalog without losing canonical selection values", () => {
		expect(buildPickerModels(available, catalog)).toEqual([
			{
				value: "deepseek/flash",
				selection: { providerID: "session", modelID: "deepseek/flash" },
				providerID: "deepseek",
				providerName: "DeepSeek",
				modelID: "flash",
				displayName: "Flash",
				ready: true,
			},
			{
				value: "openai-codex/gpt-codex",
				selection: { providerID: "session", modelID: "openai-codex/gpt-codex" },
				providerID: "openai-codex",
				providerName: "OpenAI Codex",
				modelID: "gpt-codex",
				displayName: "Coding model",
				ready: false,
			},
		])
	})
	test("matches provider and model terms together, case insensitively", () => {
		const models = buildPickerModels(available, catalog)
		expect([
			filterPickerModels(models, "", "ready"),
			filterPickerModels(models, "CODEX coding", "all"),
			filterPickerModels(models, "codex", "ready"),
			filterPickerModels(models, "missing", "all"),
		]).toEqual([[models[0]], [models[1]], [], []])
	})
	test("keeps duplicate labels and connection variant paths distinct", () => {
		const variants = {
			providers: [
				{
					id: "session",
					name: "Session",
					models: {
						"deepseek/flash/high": { name: "Coding model" },
						"other/custom/path": { name: "Coding model" },
					},
				},
			],
			defaults: {},
		} as ProvidersData
		const models = buildPickerModels(variants, null)
		expect(models.map((model) => model.selection)).toEqual([
			{ providerID: "session", modelID: "deepseek/flash/high" },
			{ providerID: "session", modelID: "other/custom/path" },
		])
	})
	test("still offers connection when there are no ready models", () => {
		expect(
			buildPickerModels(null, catalog).map((model) => ({
				value: model.value,
				ready: model.ready,
			})),
		).toEqual([
			{ value: "deepseek/flash", ready: true },
			{ value: "openai-codex/gpt-codex", ready: false },
		])
	})
	test("merges remote models and recognizes credential-only providers", () => {
		const directory: ModelDirectoryData = {
			models: [
				{
					slug: "openai-codex/remote-new",
					displayName: "Remote New",
					providerId: "openai-codex",
					modelId: "remote-new",
					provider: "openai_responses",
					contextWindow: 200000,
					reasoningCapability: { levels: [] },
					inputModalities: ["text"],
				},
			],
			credentials: [
				{
					id: "codex-auth",
					provider: "openai-codex",
					masked: "***",
					kind: "oauth",
				},
			],
		}
		expect(buildPickerModels(null, catalog, directory)).toEqual([
			{
				value: "deepseek/flash",
				selection: { providerID: "session", modelID: "deepseek/flash" },
				providerID: "deepseek",
				providerName: "DeepSeek",
				modelID: "flash",
				displayName: "Flash",
				ready: true,
			},
			{
				value: "openai-codex/gpt-codex",
				selection: { providerID: "session", modelID: "openai-codex/gpt-codex" },
				providerID: "openai-codex",
				providerName: "OpenAI Codex",
				modelID: "gpt-codex",
				displayName: "Coding model",
				ready: true,
			},
			{
				value: "openai-codex/remote-new",
				selection: {
					providerID: "session",
					modelID: "openai-codex/remote-new",
				},
				providerID: "openai-codex",
				providerName: "OpenAI Codex",
				modelID: "remote-new",
				displayName: "Remote New",
				ready: true,
			},
		])
	})
})

test("a Native preference placeholder does not prove its provider is configured", async () => {
	const requests: string[] = []
	const transport: DevoNativeTransport = {
		connected: () => true,
		subscribe: () => () => {},
		respond: async () => {},
		request: async (method) => {
			requests.push(method)
			if (method === "initialize")
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			if (method === "model/preferences/read")
				return {
					preferences: {
						model: "302ai/MiniMax-M1",
						availableModels: [{ value: "302ai/MiniMax-M1", label: "302ai/MiniMax-M1" }],
						availableEfforts: [],
					},
				}
			throw new Error(`Unexpected request ${method}`)
		},
	}
	const client = createDevoClient({ directory: "/repo", transport })
	const { data } = await client.config.providers()
	const unconfigured: ProviderCatalogData = {
		providers: [
			{
				id: "302ai",
				name: "302.AI",
				enabled: true,
				wireApis: ["openai_chat_completions"],
				models: { "MiniMax-M1": { name: "MiniMax-M1" } },
			},
		],
		connectedIds: new Set(),
		templateIds: new Set(["302ai"]),
		connectionModels: {},
	}
	const models = buildPickerModels(
		{ providers: data.providers, defaults: data.default },
		unconfigured,
	)
	expect(models).toEqual([
		{
			value: "302ai/MiniMax-M1",
			selection: { providerID: "session", modelID: "302ai/MiniMax-M1" },
			providerID: "302ai",
			providerName: "302.AI",
			modelID: "MiniMax-M1",
			displayName: "MiniMax-M1",
			ready: false,
		},
	])
	expect(filterPickerModels(models, "", "ready")).toEqual([])
	expect(filterPickerModels(models, "", "all")).toEqual(models)
	expect(requests).toEqual(["initialize", "model/preferences/read"])
})

test("stale Native preferences lose readiness after the connection is removed", () => {
	const disconnected = { ...catalog, connectedIds: new Set<string>() }
	expect(
		buildPickerModels(available, catalog).find((model) => model.value === "deepseek/flash")?.ready,
	).toBe(true)
	expect(
		buildPickerModels(available, disconnected).find((model) => model.value === "deepseek/flash")
			?.ready,
	).toBe(false)
})

test("credential-only Native models keep their exact variant selection", () => {
	const preferred = {
		...available,
		providers: [
			{ ...available.providers[0], models: { "deepseek/flash/high": { name: "Flash high" } } },
		],
	} as ProvidersData
	const directory: ModelDirectoryData = {
		models: [],
		credentials: [{ id: "deepseek-auth", provider: "deepseek", masked: "***", kind: "api_key" }],
	}
	expect(buildPickerModels(preferred, null, directory)).toEqual([
		{
			value: "deepseek/flash/high",
			selection: { providerID: "session", modelID: "deepseek/flash/high" },
			providerID: "deepseek",
			providerName: "Session",
			modelID: "flash/high",
			displayName: "Flash high",
			ready: true,
		},
	])
	expect(buildPickerModels(preferred, null)[0]?.ready).toBe(false)
})

test("unqualified Native default-route values retain their existing readiness", () => {
	const direct = {
		...available,
		providers: [
			{ ...available.providers[0], models: { "default-model": { name: "Default model" } } },
		],
	} as ProvidersData
	expect(
		buildPickerModels(direct, null).map((model) => ({
			selection: model.selection,
			ready: model.ready,
		})),
	).toEqual([{ selection: { providerID: "session", modelID: "default-model" }, ready: true }])
})
