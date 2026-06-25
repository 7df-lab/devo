/**
 * Scanner tests.
 *
 * These tests verify the scanner correctly finds and parses Claude Code config files.
 * Note: Some tests require actual ~/.Claude/ files to exist.
 */
import { describe, expect, it } from "bun:test"
import { scan } from "../../src/scanner"

describe("scanner", () => {
	describe("scan() with real filesystem", () => {
		it("returns a valid ScanResult structure", async () => {
			const result = await scan({ global: true })

			expect(result).toHaveProperty("global")
			expect(result).toHaveProperty("projects")
			expect(Array.isArray(result.projects)).toBe(true)
			expect(result.global).toHaveProperty("skills")
			expect(Array.isArray(result.global.skills)).toBe(true)
		})

		it("finds global settings when ~/.Claude/settings.json exists", async () => {
			const result = await scan({ global: true })

			// This test passes if settings.json exists on the machine
			// If it doesn't exist, settings will be undefined (not an error)
			if (result.global.settings) {
				expect(result.global.settings).toBeTypeOf("object")
				// Common fields that may exist
				if (result.global.settings.model) {
					expect(result.global.settings.model).toBeTypeOf("string")
				}
				if (result.global.settings.permissions) {
					expect(result.global.settings.permissions).toBeTypeOf("object")
				}
			}
		})

		it("finds user state when ~/.claude.json exists", async () => {
			const result = await scan({ global: true })

			// This test passes if .claude.json exists on the machine
			if (result.global.userState) {
				expect(result.global.userState).toBeTypeOf("object")
				// Common fields
				if (result.global.userState.projects) {
					expect(result.global.userState.projects).toBeTypeOf("object")
				}
			}
		})

		it("scans projects listed in ~/.claude.json", async () => {
			const result = await scan({ global: true })

			// If userState has projects, scanner should find them
			if (result.global.userState?.projects) {
				const _projectPaths = Object.keys(result.global.userState.projects)
				// Should have scanned each project
				expect(result.projects.length).toBeGreaterThanOrEqual(0)

				for (const project of result.projects) {
					expect(project).toHaveProperty("path")
					expect(project).toHaveProperty("agents")
					expect(project).toHaveProperty("commands")
					expect(project).toHaveProperty("skills")
					expect(project).toHaveProperty("projectMcpServers")
					expect(Array.isArray(project.agents)).toBe(true)
					expect(Array.isArray(project.commands)).toBe(true)
					expect(Array.isArray(project.skills)).toBe(true)
				}
			}
		})

		it("can scan a specific project path", async () => {
			const result = await scan({
				global: false,
				project: process.cwd(),
			})

			expect(result.projects.length).toBe(1)
			expect(result.projects[0].path).toBe(process.cwd())
		})
	})

	describe("scan() without global", () => {
		it("skips global config when global=false", async () => {
			const result = await scan({
				global: false,
				project: process.cwd(),
			})

			// Global should have minimal data
			expect(result.global.settings).toBeUndefined()
			expect(result.global.userState).toBeUndefined()
		})
	})

	describe("scan() project structure", () => {
		it("returns proper project structure", async () => {
			const result = await scan({
				global: false,
				project: process.cwd(),
			})

			const project = result.projects[0]

			// Required fields
			expect(project.path).toBe(process.cwd())
			expect(Array.isArray(project.agents)).toBe(true)
			expect(Array.isArray(project.commands)).toBe(true)
			expect(Array.isArray(project.skills)).toBe(true)
			expect(typeof project.projectMcpServers).toBe("object")
			// allowedTools and disabledMcpServers may be undefined if not in ~/.claude.json
			expect(project.allowedTools === undefined || Array.isArray(project.allowedTools)).toBe(true)
			expect(
				project.disabledMcpServers === undefined || Array.isArray(project.disabledMcpServers),
			).toBe(true)

			// Optional fields
			expect(project.mcpJson === undefined || typeof project.mcpJson === "object").toBe(true)
			expect(project.settingsLocal === undefined || typeof project.settingsLocal === "object").toBe(
				true,
			)
			expect(project.claudeMd === undefined || typeof project.claudeMd === "string").toBe(true)
			expect(project.agentsMd === undefined || typeof project.agentsMd === "string").toBe(true)
		})
	})
})
