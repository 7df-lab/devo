import { describe, expect, test } from "bun:test"
import { settingsGroups } from "./settings-navigation"

describe("Settings navigation groups", () => {
	test("keeps every settings route in exactly one named group", () => {
		const routeIds = settingsGroups.flatMap((group) => group.tabs.map((tab) => tab.id))

		expect(settingsGroups.map((group) => group.label)).toEqual(["App", "Connections", "Workspace"])
		expect(routeIds).toEqual([
			"general",
			"notifications",
			"about",
			"servers",
			"providers",
			"mcp",
			"skills",
			"worktrees",
			"setup",
		])
		expect(new Set(routeIds).size).toBe(routeIds.length)
	})
})
