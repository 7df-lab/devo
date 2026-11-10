import { expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "./client"

for (const directory of [undefined, "/workspace/project"]) {
	test(`availability is a fresh Native read for ${directory ?? "global defaults"}`, async () => {
		const requests: unknown[] = []
		let available = [{ value: "workspace/tiny", label: "Tiny" }]
		const transport: DevoNativeTransport = {
			connected: () => true,
			subscribe: () => () => {},
			respond: async () => {},
			request: async (method, params) => {
				if (method === "initialize")
					return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
				if (method !== "model/preferences/read") throw Error(method)
				requests.push(params)
				return {
					preferences: {
						model: "workspace/tiny",
						availableModels: available,
						availableEfforts: [],
					},
				}
			},
		}
		const client = createDevoClient({ directory, transport })
		expect((await client.config.providers()).data.providers[0].models).toHaveProperty(
			"workspace/tiny",
		)
		await client.config.get() // Populate the old config-option cache too.
		available = []
		expect((await client.config.providers()).data.providers).toEqual([])
		expect(requests).toEqual(Array(3).fill(directory ? { cwd: directory } : {}))
	})
}

test("a failed Native availability read never falls back to cached config options", async () => {
	let fail = false
	const transport: DevoNativeTransport = {
		connected: () => true,
		subscribe: () => () => {},
		respond: async () => {},
		request: async (method) => {
			if (method === "initialize")
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			if (method !== "model/preferences/read") throw Error(method)
			if (fail) throw Error("Native availability unavailable")
			return {
				preferences: {
					model: "old/model",
					availableModels: [{ value: "old/model", label: "Old" }],
					availableEfforts: [],
				},
			}
		},
	}
	const client = createDevoClient({ directory: "/workspace/project", transport })
	await client.config.get()
	fail = true
	await expect(client.config.providers()).rejects.toThrow("Native availability unavailable")
})
