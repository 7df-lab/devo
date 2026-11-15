import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { desktopFolderNameFromDirectory } from "./desktop-folders"

describe("automation project display labels", () => {
	// Display strings may come from a remote host, irrespective of the test host OS.
	for (const [directory, label] of [
		["C:\\Users\\jack\\devo", "devo"],
		["C:\\Users\\jack\\devo\\", "devo"],
		["C:/Users/jack/devo/", "devo"],
		["/repo/devo/", "devo"],
		["/", "/"],
		["C:\\", "C:"],
		["\\\\server\\share\\devo", "devo"],
		["/repo/项目", "项目"],
	]) {
		test(`labels ${directory} without changing the directory value`, () => {
			expect(desktopFolderNameFromDirectory(directory)).toBe(label)
		})
	}

	test("all automation project labels use the shared display helper", () => {
		for (const file of ["automation-row", "automation-detail", "create-automation-dialog", "inbox-run-list"]) {
			const source = readFileSync(new URL(`../components/automations/${file}.tsx`, import.meta.url), "utf8")
			expect(source).toContain("desktopFolderNameFromDirectory(")
			expect(source).not.toContain('.split("/").pop()')
		}
	})
})
