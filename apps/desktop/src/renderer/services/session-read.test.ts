import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { getSession } from "./devo"

describe("session read error boundary", () => {
	for (const scenario of [
		{
			name: "missing session",
			error: Object.assign(new Error("gone"), { code: "SessionNotFound" }),
			missing: true,
		},
		{ name: "transport error", error: new Error("Connection lost"), missing: false },
		{
			name: "permission error",
			error: Object.assign(new Error("Access denied"), { code: "PermissionDenied" }),
			missing: false,
		},
	]) {
		test(scenario.name, async () => {
			const requests: Array<{ method: string; params: unknown }> = []
			const transport: DevoNativeTransport = {
				async request(method, params) {
					requests.push({ method, params })
					if (method === "initialize")
						return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
					throw scenario.error
				},
				async respond() {},
				subscribe: () => () => {},
				connected: () => true,
			}
			const client = createDevoClient({ transport })
			try {
				if (scenario.missing) expect(await getSession(client, "child")).toBeNull()
				else await expect(getSession(client, "child")).rejects.toThrow(scenario.error.message)
				expect(requests.filter(({ method }) => method !== "initialize")).toEqual([
					{ method: "session/read", params: { sessionId: "child" } },
				])
			} finally {
				client.dispose()
			}
		})
	}
})
