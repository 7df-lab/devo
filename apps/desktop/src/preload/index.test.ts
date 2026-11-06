import { describe, expect, mock, test } from "bun:test"
import type { DevoAPI } from "./api"

let bridge: DevoAPI
const invoke = mock(async (..._args: unknown[]) => {})
mock.module("electron", () => ({
	contextBridge: {
		exposeInMainWorld: (name: string, api: DevoAPI) => {
			expect(name).toBe("devo")
			bridge = api
		},
	},
	ipcRenderer: { invoke },
}))
await import("./index")

describe("Desktop preload surface", () => {
	test("does not expose the obsolete password store or duplicate workspace diff APIs", () => {
		expect({
			credential: "credential" in bridge,
			summary: "workspaceChangesSummary" in bridge.git,
			patch: "workspaceFilePatch" in bridge.git,
		}).toEqual({ credential: false, summary: false, patch: false })
	})

	test("retains real git operations and the selected runtime check", async () => {
		await bridge.git.checkout("repo", "main")
		await bridge.onboarding.checkDevo()
		expect(invoke.mock.calls).toEqual([
			["git:checkout", "repo", "main"],
			["onboarding:check-devo"],
		])
	})
})
