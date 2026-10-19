import { expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "./client"

test("worktree lifecycle projects real Native results instead of placeholder success", async () => {
	const calls: Array<{ method: string; params: unknown }> = []
	const cwd = "C:/QA/repository"
	const directory = "C:/QA/worktree"
	const info = { directory, name: "qa", branch: "devo/qa" }
	const transport: DevoNativeTransport = { connected: () => true, respond: async () => {}, subscribe: () => () => {},
		request: async (method, params) => {
			calls.push({ method, params })
			if (method === "initialize") return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			if (method === "workspace/worktree/list") return { worktrees: [info] }
			if (method === "workspace/worktree/create") return info
			return { directory }
		} }
	const client = createDevoClient({ transport, directory: cwd })
	expect([await client.worktree.list(), await client.worktree.create({ worktreeCreateInput: { name: "qa" } }),
		await client.worktree.reset({ worktreeResetInput: { directory } }), await client.worktree.remove({ worktreeRemoveInput: { directory } })])
		.toEqual([{ data: [directory] }, { data: info }, { data: { directory } }, { data: { directory } }])
	expect(calls.filter((call) => call.method !== "initialize")).toEqual([
		{ method: "workspace/worktree/list", params: { cwd } },
		{ method: "workspace/worktree/create", params: { cwd, name: "qa" } },
		{ method: "workspace/worktree/reset", params: { cwd, directory } },
		{ method: "workspace/worktree/remove", params: { cwd, directory } } ])
})
