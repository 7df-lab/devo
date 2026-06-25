/**
 * Writer integration test.
 *
 * Tests the write() function with dry-run mode and actual writes to temp directories.
 */
import { afterEach, describe, expect, it } from "bun:test"
import { mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { ConversionResult } from "../../src/types/conversion-result"
import { createEmptyReport } from "../../src/types/report"
import { write } from "../../src/writer"

function makeConversion(overrides: Partial<ConversionResult> = {}): ConversionResult {
	return {
		globalConfig: overrides.globalConfig ?? {},
		projectConfigs: overrides.projectConfigs ?? new Map(),
		agents: overrides.agents ?? new Map(),
		commands: overrides.commands ?? new Map(),
		rules: overrides.rules ?? new Map(),
		hookPlugins: overrides.hookPlugins ?? new Map(),
		report: overrides.report ?? createEmptyReport(),
	}
}

// Use unique temp directories per test to avoid collisions
function tempDir(): string {
	return join(tmpdir(), `cc2oc-test-${Date.now()}-${Math.random().toString(36).slice(2)}`)
}

describe("write()", () => {
	const tempDirs: string[] = []

	afterEach(async () => {
		for (const dir of tempDirs) {
			try {
				await rm(dir, { recursive: true })
			} catch {}
		}
		tempDirs.length = 0
	})

	describe("dry-run mode", () => {
		it("reports files that would be written without touching disk", async () => {
			const dir = tempDir()
			tempDirs.push(dir)

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/reviewer.md`, "# Reviewer\nContent")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, { dryRun: true })

			expect(result.filesWritten.length).toBe(1)
			expect(result.filesWritten[0]).toContain("reviewer.md")
			expect(result.filesSkipped.length).toBe(0)

			// File should NOT actually exist
			try {
				await readFile(`${dir}/agents/reviewer.md`, "utf-8")
				throw new Error("File should not exist in dry-run")
			} catch (err) {
				expect((err as NodeJS.ErrnoException).code).toBe("ENOENT")
			}
		})
	})

	describe("actual writes", () => {
		it("writes agent files to disk", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "agents"), { recursive: true })

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/reviewer.md`, "# Reviewer\nReviews code")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, { dryRun: false, force: true })

			expect(result.filesWritten.length).toBe(1)

			const content = await readFile(`${dir}/agents/reviewer.md`, "utf-8")
			expect(content).toBe("# Reviewer\nReviews code")
		})

		it("writes command files to disk", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "commands"), { recursive: true })

			const commands = new Map<string, string>()
			commands.set(`${dir}/commands/deploy.md`, "# Deploy\nDeploy command")

			const conversion = makeConversion({ commands })

			const result = await write(conversion, { dryRun: false, force: true })

			expect(result.filesWritten.length).toBe(1)
			const content = await readFile(`${dir}/commands/deploy.md`, "utf-8")
			expect(content).toBe("# Deploy\nDeploy command")
		})

		it("writes rules files to disk", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(dir, { recursive: true })

			const rules = new Map<string, string>()
			rules.set(`${dir}/AGENTS.md`, "# Rules\nUse TypeScript")

			const conversion = makeConversion({ rules })

			const result = await write(conversion, { dryRun: false, force: true })

			expect(result.filesWritten.length).toBe(1)
			const content = await readFile(`${dir}/AGENTS.md`, "utf-8")
			expect(content).toBe("# Rules\nUse TypeScript")
		})

		it("writes hook plugin files to disk", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "plugins"), { recursive: true })

			const hookPlugins = new Map<string, string>()
			hookPlugins.set(
				`${dir}/plugins/cc-hooks.ts`,
				'export default () => ({ "tool.execute.before": async () => {} })',
			)

			const conversion = makeConversion({ hookPlugins })

			const result = await write(conversion, { dryRun: false, force: true })

			expect(result.filesWritten.length).toBe(1)
		})
	})

	describe("skip existing", () => {
		it("skips files that already exist when force is false", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "agents"), { recursive: true })

			// Pre-create the file
			await writeFile(`${dir}/agents/existing.md`, "Original content")

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/existing.md`, "New content")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, { dryRun: false, force: false })

			expect(result.filesSkipped.length).toBe(1)
			expect(result.filesWritten.length).toBe(0)

			// Original content preserved
			const content = await readFile(`${dir}/agents/existing.md`, "utf-8")
			expect(content).toBe("Original content")
		})

		it("overwrites existing files when force is true", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "agents"), { recursive: true })

			await writeFile(`${dir}/agents/existing.md`, "Original content")

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/existing.md`, "New content")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, { dryRun: false, force: true })

			expect(result.filesWritten.length).toBe(1)
			const content = await readFile(`${dir}/agents/existing.md`, "utf-8")
			expect(content).toBe("New content")
		})
	})

	describe("backups", () => {
		it("creates backup snapshot when overwriting with backup=true", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "agents"), { recursive: true })

			await writeFile(`${dir}/agents/existing.md`, "Original content")

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/existing.md`, "New content")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, {
				dryRun: false,
				force: true,
				backup: true,
			})

			expect(result.filesWritten.length).toBe(1)

			// New backup behavior: backupDir is set, backupPaths is empty (legacy)
			expect(result.backupDir).toBeDefined()
			expect(result.backupDir).toContain("backups")
			expect(result.backupPaths.length).toBe(0)

			// Verify manifest exists and has correct content
			const manifestContent = await readFile(join(result.backupDir!, "manifest.json"), "utf-8")
			const manifest = JSON.parse(manifestContent)
			expect(manifest.files.length).toBeGreaterThanOrEqual(1)

			const entry = manifest.files.find(
				(f: { originalPath: string }) => f.originalPath === `${dir}/agents/existing.md`,
			)
			expect(entry).toBeDefined()
			expect(entry.existedBefore).toBe(true)

			// Verify backup file has original content
			const backupContent = await readFile(
				join(result.backupDir!, "files", entry.backupFilename),
				"utf-8",
			)
			expect(backupContent).toBe("Original content")

			// New file has new content
			const newContent = await readFile(`${dir}/agents/existing.md`, "utf-8")
			expect(newContent).toBe("New content")
		})

		it("does not create backup in dry-run mode", async () => {
			const dir = tempDir()
			tempDirs.push(dir)
			await mkdir(join(dir, "agents"), { recursive: true })

			await writeFile(`${dir}/agents/existing.md`, "Original content")

			const agents = new Map<string, string>()
			agents.set(`${dir}/agents/existing.md`, "New content")

			const conversion = makeConversion({ agents })

			const result = await write(conversion, {
				dryRun: true,
				force: true,
				backup: true,
			})

			expect(result.backupDir).toBeUndefined()

			// Original file untouched
			const content = await readFile(`${dir}/agents/existing.md`, "utf-8")
			expect(content).toBe("Original content")
		})
	})

	describe("empty conversion", () => {
		it("writes nothing for empty conversion result", async () => {
			const conversion = makeConversion()

			const result = await write(conversion, { dryRun: false })

			expect(result.filesWritten.length).toBe(0)
			expect(result.filesSkipped.length).toBe(0)
			expect(result.backupDir).toBeUndefined()
		})
	})
})
