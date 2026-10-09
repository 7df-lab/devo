import { beforeEach, describe, expect, mock, test } from "bun:test"

const write = mock(async () => undefined)
const invalidateQueries = mock(async () => undefined)

mock.module("../services/connection-manager", () => ({
	getBaseClient: () => null,
	getProjectClient: () => ({
		model: { preferences: { write } },
	}),
}))

mock.module("./query-client", () => ({
	queryClient: { invalidateQueries },
}))

const { persistRuntimeModelConfigOption, persistRuntimeModelSelection } = await import(
	"./model-config-options"
)

describe("runtime model config option persistence", () => {
	beforeEach(() => {
		write.mockClear()
		invalidateQueries.mockClear()
	})

	test("persists selected model through runtime config", async () => {
		await persistRuntimeModelSelection("/repo", {
			providerID: "session",
			modelID: "deepseek-v4-flash",
		})

		expect(write).toHaveBeenCalledWith({
			patch: { model: "deepseek-v4-flash" },
		})
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["providers", "/repo"] })
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["config", "/repo"] })
	})

	test("persists selected reasoning effort through runtime config", async () => {
		await persistRuntimeModelConfigOption("/repo", "thought_level", "max")

		expect(write).toHaveBeenCalledWith({
			patch: { reasoningEffort: "max" },
		})
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["providers", "/repo"] })
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["config", "/repo"] })
	})
})


test("non-session providers persist their qualified model identity", async () => {
	await persistRuntimeModelSelection("/repo", { providerID: "openai-codex", modelID: "gpt-6-luna" })
	expect(write).toHaveBeenLastCalledWith({ patch: { model: "openai-codex/gpt-6-luna" } })
	expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["modelDefaults"] })
})
