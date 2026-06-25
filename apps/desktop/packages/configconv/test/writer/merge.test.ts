import { describe, expect, test } from "bun:test"
import type { DevoConfig } from "../../src/types/devo"
import { mergeConfigs } from "../../src/writer/merge"

describe("mergeConfigs", () => {
	test("overwrite strategy replaces all top-level keys", () => {
		const existing: Partial<DevoConfig> = {
			model: "anthropic/old-model",
			theme: "dark",
		}
		const incoming: Partial<DevoConfig> = {
			model: "anthropic/new-model",
			autoupdate: true,
		}

		const result = mergeConfigs(existing, incoming, "overwrite")
		expect(result.model).toBe("anthropic/new-model")
		expect(result.theme).toBe("dark") // from existing spread
		expect(result.autoupdate).toBe(true)
	})

	test("preserve-existing keeps existing scalar values", () => {
		const existing: Partial<DevoConfig> = {
			model: "anthropic/existing-model",
		}
		const incoming: Partial<DevoConfig> = {
			model: "anthropic/new-model",
			autoupdate: true,
		}

		const result = mergeConfigs(existing, incoming, "preserve-existing")
		expect(result.model).toBe("anthropic/existing-model") // preserved
		expect(result.autoupdate).toBe(true) // added (didn't exist)
	})

	test("preserve-existing merges nested objects at key level", () => {
		const existing: Partial<DevoConfig> = {
			mcp: {
				server1: { type: "local", command: ["node", "existing.js"] },
			},
		}
		const incoming: Partial<DevoConfig> = {
			mcp: {
				server1: { type: "local", command: ["node", "new.js"] }, // should be skipped
				server2: { type: "local", command: ["node", "new2.js"] }, // should be added
			},
		}

		const result = mergeConfigs(existing, incoming, "preserve-existing")
		const mcp = result.mcp as Record<string, unknown>
		expect(mcp.server1).toEqual({ type: "local", command: ["node", "existing.js"] })
		expect(mcp.server2).toEqual({ type: "local", command: ["node", "new2.js"] })
	})

	test("merge strategy deep-merges with existing precedence", () => {
		const existing: Partial<DevoConfig> = {
			model: "anthropic/existing",
			mcp: {
				server1: { type: "local", command: ["node", "existing.js"] },
			},
		}
		const incoming: Partial<DevoConfig> = {
			model: "anthropic/new",
			autoupdate: true,
			mcp: {
				server2: { type: "local", command: ["node", "new.js"] },
			},
		}

		const result = mergeConfigs(existing, incoming, "merge")
		expect(result.model).toBe("anthropic/existing") // existing takes precedence
		expect(result.autoupdate).toBe(true) // new key added
		const mcp = result.mcp as Record<string, unknown>
		expect(mcp.server1).toBeDefined() // preserved
		expect(mcp.server2).toBeDefined() // added
	})

	test("merge deduplicates arrays", () => {
		const existing: Partial<DevoConfig> = {
			instructions: ["rule1.md", "rule2.md"],
		}
		const incoming: Partial<DevoConfig> = {
			instructions: ["rule2.md", "rule3.md"],
		}

		const result = mergeConfigs(existing, incoming, "merge")
		expect(result.instructions).toEqual(["rule1.md", "rule2.md", "rule3.md"])
	})

	test("defaults to preserve-existing strategy", () => {
		const existing: Partial<DevoConfig> = { model: "anthropic/existing" }
		const incoming: Partial<DevoConfig> = { model: "anthropic/new" }

		const result = mergeConfigs(existing, incoming)
		expect(result.model).toBe("anthropic/existing")
	})

	test("handles empty existing config", () => {
		const incoming: Partial<DevoConfig> = {
			model: "anthropic/new",
			autoupdate: true,
		}

		const result = mergeConfigs({}, incoming)
		expect(result.model).toBe("anthropic/new")
		expect(result.autoupdate).toBe(true)
	})

	test("handles empty incoming config", () => {
		const existing: Partial<DevoConfig> = {
			model: "anthropic/existing",
		}

		const result = mergeConfigs(existing, {})
		expect(result.model).toBe("anthropic/existing")
	})
})
