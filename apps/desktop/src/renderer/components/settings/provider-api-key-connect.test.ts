import { describe, expect, test } from "bun:test"
import type { CatalogProvider } from "../../hooks/use-devo-data"
import { connectProviderWithApiKey } from "./provider-api-key-connect"

type ConnectionClient = Parameters<typeof connectProviderWithApiKey>[0]

function makeClient() {
	const calls: unknown[] = []
	const client = {
		provider: {
			upsert: async (params: unknown) => {
				calls.push(params)
				return { data: {} }
			},
		},
	} as ConnectionClient
	return { client, calls }
}

const template = {
	id: "anthropic",
	name: "Anthropic",
	baseUrl: "https://api.anthropic.com",
	credential: "anthropic",
	wireApis: ["anthropic_messages"],
	models: { "claude-sonnet": { name: "Claude Sonnet" } },
	options: { existing: "kept", baseURL: "https://original.example" },
	enabled: true,
	env: ["ANTHROPIC_API_KEY"],
} as unknown as CatalogProvider

describe("connectProviderWithApiKey", () => {
	test("connects Anthropic and xAI with the native write-only API key and full template", async () => {
		for (const id of ["anthropic", "xai"]) {
			const { client, calls } = makeClient()
			await connectProviderWithApiKey(client, { ...template, id }, "  secret-key  ")
			const { env: _env, ...catalogFields } = template
			expect(calls).toEqual([{
				provider: { ...catalogFields, id },
				apiKey: "secret-key",
			}])
			expect(JSON.stringify((calls[0] as { provider: unknown }).provider)).not.toContain("secret-key")
		}
	})

	test("preserves template options when saving structured provider options", async () => {
		const { client, calls } = makeClient()
		const { env: _env, ...catalogFields } = template
		await connectProviderWithApiKey(client, template, " azure-secret ", {
			baseURL: "https://azure.example",
			region: "us-east-1",
		})
		expect(calls).toEqual([{
			provider: {
				...catalogFields,
				options: { existing: "kept", baseURL: "https://azure.example", region: "us-east-1" },
			},
			apiKey: "azure-secret",
		}])
		expect(template.options).toEqual({ existing: "kept", baseURL: "https://original.example" })
	})

	test("propagates native failures instead of reporting a false success", async () => {
		const client = {
			provider: { upsert: async () => { throw new Error("store failed") } },
		} as ConnectionClient
		expect(connectProviderWithApiKey(client, template, "secret-key")).rejects.toThrow("store failed")
	})
})
