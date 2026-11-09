import { describe, expect, test } from "bun:test"
import type { ProviderCatalogData, ProvidersData } from "../../hooks/use-devo-data"
import { buildPickerModels, filterPickerModels } from "./model-picker-data"
import type { ModelDirectoryData } from "../../hooks/use-model-directory"

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
