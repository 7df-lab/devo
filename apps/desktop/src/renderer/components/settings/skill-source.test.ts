import { describe, expect, test } from "bun:test"
import { skillSourceLabel } from "./skill-source"

describe("skill source labels", () => {
	test("normalizes tagged string variants", () => {
		expect(["User", "Workspace", "Plugin", "System", "Admin", "community"].map(skillSourceLabel)).toEqual([
			"user",
			"workspace",
			"plugin",
			"system",
			"admin",
			"community",
		])
	})

	test("normalizes tagged object variants and path-shaped values", () => {
		expect([
			skillSourceLabel({ User: null }),
			skillSourceLabel({ Workspace: { cwd: "/repo" } }),
			skillSourceLabel({ Plugin: { id: "plugin-1" } }),
			skillSourceLabel({ System: null }),
			skillSourceLabel({ Admin: null }),
			skillSourceLabel({ cwd: "/repo" }),
			skillSourceLabel({ plugin_id: "plugin-1" }),
			skillSourceLabel(null),
		]).toEqual(["user", "workspace", "plugin", "system", "admin", "workspace", "plugin", "unknown"])
	})
})
