import { expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import type { ProviderCatalogData, ProvidersData } from "../../hooks/use-devo-data"
import { buildPickerModels, filterPickerModels } from "./model-picker-data"

const emptyCatalog: ProviderCatalogData = {
	providers: [],
	connectedIds: new Set(),
	templateIds: new Set(),
	connectionModels: {},
}

async function nativeProviders(
	model: string,
	availableModels: Array<{ value: string; label: string }>,
): Promise<ProvidersData> {
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
				return { preferences: { model, availableModels, availableEfforts: [] } }
			throw new Error(`Unexpected request ${method}`)
		},
	}
	const client = createDevoClient({ directory: "/workspace/project", transport })
	const { data } = await client.config.providers()
	expect(requests).toEqual(["initialize", "model/preferences/read"])
	return { providers: data.providers, defaults: data.default }
}

test("a current preference without available Native options is not shown as ready", async () => {
	const available = await nativeProviders("302ai/MiniMax-M1", [])
	const catalog: ProviderCatalogData = {
		...emptyCatalog,
		providers: [
			{
				id: "302ai",
				name: "302.AI",
				enabled: true,
				wireApis: [],
				models: { "MiniMax-M1": { name: "MiniMax-M1" } },
			},
		],
		templateIds: new Set(["302ai"]),
	}
	const models = buildPickerModels(available, catalog)
	expect(available.providers).toEqual([])
	expect(filterPickerModels(models, "", "ready")).toEqual([])
	expect(filterPickerModels(models, "", "all")).toEqual([
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
})

test("workspace Native choices remain selectable without global connections or credentials", async () => {
	const available = await nativeProviders("reviewlocal/tiny", [
		{ value: "reviewlocal/tiny", label: "Tiny" },
	])
	const models = buildPickerModels(available, emptyCatalog, { models: [], credentials: [] })
	expect(filterPickerModels(models, "", "ready").map((model) => model.selection)).toEqual([
		{ providerID: "session", modelID: "reviewlocal/tiny" },
	])
})

for (const value of ["default-model", "organization/model", "workspace/model/high"]) {
	test(`Native route identity is preserved for ${value}`, async () => {
		const available = await nativeProviders(value, [{ value, label: "Configured model" }])
		const models = buildPickerModels(available, emptyCatalog)
		expect(models.map((model) => ({ selection: model.selection, ready: model.ready }))).toEqual([
			{ selection: { providerID: "session", modelID: value }, ready: true },
		])
	})
}

test("Native-filtered routes never return through configured catalog or credentials", async () => {
	const available = await nativeProviders("local/unusable", [])
	const catalog: ProviderCatalogData = {
		...emptyCatalog,
		providers: [
			{
				id: "local",
				name: "Local",
				enabled: true,
				baseUrl: "http://127.0.0.1:1/v1",
				wireApis: ["anthropic_messages"],
				models: { unusable: { name: "Unusable" } },
			},
		],
		connectedIds: new Set(["local"]),
	}
	const models = buildPickerModels(available, catalog, {
		models: [
			{
				slug: "local/unusable",
				providerId: "local",
				modelId: "unusable",
				displayName: "Unusable",
				provider: "anthropic_messages",
				contextWindow: 1000,
				reasoningCapability: { levels: [] },
				inputModalities: ["text"],
			},
		],
		credentials: [{ id: "old-key", provider: "local", kind: "api_key", masked: "***" }],
	})
	expect(filterPickerModels(models, "", "ready")).toEqual([])
	expect(
		filterPickerModels(models, "", "all").map(({ value, ready }) => ({ value, ready })),
	).toEqual([{ value: "local/unusable", ready: false }])
})

test("directory-only opaque defaults are not assumed to be available", () => {
	const models = buildPickerModels(null, null, {
		models: [
			{
				slug: "default-model",
				displayName: "Default",
				provider: "openai_chat_completions",
				contextWindow: 1000,
				reasoningCapability: { levels: [] },
				inputModalities: ["text"],
			},
		],
		credentials: [],
	})
	expect(models.map(({ value, ready }) => ({ value, ready }))).toEqual([
		{ value: "default-model", ready: false },
	])
})
