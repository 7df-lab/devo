import { expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "./client"

test("projects Native model/credential reads and remote refresh without legacy calls", async () => {
	const requests: Array<{ method: string; params: unknown }> = []
	const models = {
		models: [
			{
				slug: "custom/test",
				displayName: "Custom Test",
				providerId: "custom",
				modelId: "test",
				provider: "openai_responses",
				contextWindow: 128000,
				reasoningCapability: { levels: [] },
				inputModalities: ["text"],
			},
		],
	}
	const credentials = {
		credentials: [{ id: "custom-auth", provider: "custom", masked: "***", kind: "oauth" }],
	}
	const transport: DevoNativeTransport = {
		connected: () => true,
		respond: async () => {},
		subscribe: () => () => {},
		request: async (method, params) => {
			requests.push({ method, params })
			switch (method) {
				case "initialize":
					return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
				case "model/list":
					return models
				case "credential/list":
					return credentials
				case "model/catalog/refresh":
					return { status: "updated" }
				default:
					throw new Error(`Unexpected ${method}`)
			}
		},
	}
	const client = createDevoClient({ transport })
	expect(await client.model.list()).toEqual({ data: models })
	expect(await client.credential.list()).toEqual({ data: credentials })
	expect(await client.model.refreshCatalog({ policy: "force" })).toEqual({
		data: { status: "updated" },
	})
	expect(requests.filter((request) => request.method !== "initialize")).toEqual([
		{ method: "model/list", params: {} },
		{ method: "credential/list", params: {} },
		{ method: "model/catalog/refresh", params: { policy: "force" } },
	])
})
