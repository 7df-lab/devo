import { expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "./client"

for (const operation of ["revert", "unrevert"] as const) {
	test(`${operation} rejects explicitly without an interrupt or destructive rollback RPC`, async () => {
		const calls: string[] = []
		const transport: DevoNativeTransport = {
			connected: () => true,
			subscribe: () => () => {},
			respond: async () => {},
			request: async (method) => {
				calls.push(method)
				throw Error(`Unexpected RPC ${method}`)
			},
		}
		const client = createDevoClient({ directory: "/fixture/project", transport })
		await expect(client.session[operation]({ sessionId: "session" })).rejects.toThrow(
			"not supported",
		)
		expect(calls).toEqual([])
	})
}
