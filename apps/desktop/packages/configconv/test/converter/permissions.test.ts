import { describe, expect, test } from "bun:test"
import {
	convertPermissions,
	convertToolListToPermissions,
	mapToolName,
	parseToolPattern,
} from "../../src/converter/permissions"

describe("parseToolPattern", () => {
	test("parses tool with pattern", () => {
		expect(parseToolPattern("Bash(git *)")).toEqual({
			toolName: "Bash",
			pattern: "git *",
		})
	})

	test("parses tool with file path pattern", () => {
		expect(parseToolPattern("Edit(docs/**)")).toEqual({
			toolName: "Edit",
			pattern: "docs/**",
		})
	})

	test("parses bare tool name", () => {
		expect(parseToolPattern("Read")).toEqual({
			toolName: "Read",
			pattern: "*",
		})
	})

	test("returns null for invalid patterns", () => {
		expect(parseToolPattern("")).toBeNull()
		expect(parseToolPattern("not a tool pattern ()")).toBeNull()
	})
})

describe("mapToolName", () => {
	test("maps known Claude Code tools to Devo", () => {
		expect(mapToolName("Read")).toBe("read")
		expect(mapToolName("Write")).toBe("write")
		expect(mapToolName("Edit")).toBe("edit")
		expect(mapToolName("MultiEdit")).toBe("edit")
		expect(mapToolName("Bash")).toBe("bash")
		expect(mapToolName("Glob")).toBe("glob")
		expect(mapToolName("Grep")).toBe("grep")
		expect(mapToolName("WebFetch")).toBe("webfetch")
		expect(mapToolName("Task")).toBe("task")
	})

	test("returns null for unknown tools", () => {
		expect(mapToolName("UnknownTool")).toBeNull()
		expect(mapToolName("CustomPlugin")).toBeNull()
	})
})

describe("convertPermissions", () => {
	test("returns default ask when no permissions", () => {
		const { permission } = convertPermissions()
		expect(permission["*"]).toBe("ask")
	})

	test("converts bypass mode to allow all", () => {
		const { permission } = convertPermissions({
			defaultMode: "bypassPermissions",
		})
		expect(permission["*"]).toBe("allow")
	})

	test("converts allow list", () => {
		const { permission, report } = convertPermissions({
			allow: ["Bash(npm run build)", "Read"],
		})
		expect(permission.bash).toEqual({ "npm run build": "allow" })
		expect(permission.read).toBe("allow")
		expect(report.migrated.length).toBe(2)
	})

	test("converts deny list", () => {
		const { permission } = convertPermissions({
			deny: ["Bash(rm -rf *)"],
		})
		expect(permission.bash).toEqual({ "rm -rf *": "deny" })
	})

	test("merges allow and deny for same tool", () => {
		const { permission } = convertPermissions({
			allow: ["Bash(git *)"],
			deny: ["Bash(rm -rf *)"],
		})
		expect(permission.bash).toEqual({
			"git *": "allow",
			"rm -rf *": "deny",
		})
	})

	test("processes per-project allowedTools", () => {
		const { permission } = convertPermissions(undefined, ["Read", "Edit", "Bash(npm run *)"])
		expect(permission.read).toBe("allow")
		expect(permission.edit).toBe("allow")
	})

	test("simplifies single wildcard patterns", () => {
		const { permission } = convertPermissions({
			allow: ["Read"],
		})
		// Should be simplified from {"*": "allow"} to just "allow"
		expect(permission.read).toBe("allow")
	})

	test("warns on unknown tool names", () => {
		const { report } = convertPermissions({
			allow: ["UnknownTool(something)"],
		})
		expect(report.warnings.length).toBeGreaterThan(0)
		expect(report.warnings[0]).toContain("Unknown tool name")
	})

	test("warns on unparseable patterns", () => {
		const { report } = convertPermissions({
			allow: ["not a valid pattern ()"],
		})
		expect(report.warnings.length).toBeGreaterThan(0)
	})
})

describe("convertToolListToPermissions", () => {
	test("converts comma-separated tool list", () => {
		const result = convertToolListToPermissions("Read, Edit, Bash, Grep")
		expect(result.read).toBe("allow")
		expect(result.edit).toBe("allow")
		expect(result.bash).toBe("ask") // Bash defaults to ask for safety
		expect(result.grep).toBe("allow")
	})

	test("handles empty string", () => {
		const result = convertToolListToPermissions("")
		expect(Object.keys(result)).toEqual([])
	})

	test("ignores unknown tools silently", () => {
		const result = convertToolListToPermissions("Read, CustomTool")
		expect(Object.keys(result)).toEqual(["read"])
	})
})
