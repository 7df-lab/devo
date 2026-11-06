import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { getWorkspaceChanges } from "./devo"

describe("workspace changes Native transport", () => {
	for (const options of [
		{ diffDetail: "summary" as const },
		{ diffDetail: "full" as const },
		{ diffDetail: "full" as const, paths: ["src/main.ts"], includeFileSides: true },
	]) {
		test(`routes ${JSON.stringify(options)} through workspace/changes/read`, async () => {
			const requests: Array<{ method: string; params: unknown }> = []
			const transport: DevoNativeTransport = {
				async request(method, params) {
					requests.push({ method, params })
					if (method === "initialize") {
						return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
					}
					if (method === "workspace/changes/read") return { views: [] }
					throw new Error(`Unexpected RPC: ${method}`)
				},
				async respond() {},
				subscribe: () => () => {},
				connected: () => true,
			}
			const client = createDevoClient({ transport })
			const params = { sessionId: "session-1", scopes: ["uncommitted" as const], ...options }
			expect(await getWorkspaceChanges(client, params)).toEqual({ views: [] })
			expect(requests.filter(({ method }) => method !== "initialize")).toEqual([
				{ method: "workspace/changes/read", params },
			])
		})
	}
})
