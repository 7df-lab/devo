/**
 * Backup module tests.
 *
 * Tests createBackup, listBackups, restore, and deleteBackup using temp directories.
 * We mock the paths module so backups go to a temp dir instead of ~/.config/devo/.
 */
import { afterEach, beforeEach, describe, expect, it, mock } from "bun:test"
import { mkdir, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

// ─── Test helpers ────────────────────────────────────────────────────

function uniqueTmpDir(): string {
	return join(tmpdir(), `cc2oc-backup-test-${Date.now()}-${Math.random().toString(36).slice(2)}`)
}

let testDir: string
let backupsDir: string
let filesDir: string

// We need to mock ocBackupsDir to point to our temp dir.
// Since Bun's mock.module has limitations, we'll test the backup module
// by invoking its functions directly and verifying the output.
// We use a lower-level approach: import the internals and pass controlled paths.

// The backup module uses paths.ocBackupsDir() internally, so we mock it.
mock.module("../../src/utils/paths", () => ({
	ocBackupsDir: () => backupsDir,
}))

// Import AFTER mocking
const { createBackup, deleteBackup, listBackups, restore } = await import("../../src/backup")

beforeEach(async () => {
	testDir = uniqueTmpDir()
	backupsDir = join(testDir, "backups")
	filesDir = join(testDir, "source-files")
	await mkdir(filesDir, { recursive: true })
})

afterEach(async () => {
	try {
		await rm(testDir, { recursive: true })
	} catch {}
})

// ─── createBackup ────────────────────────────────────────────────────

describe("createBackup()", () => {
	it("returns undefined for empty target list", async () => {
		const result = await createBackup([], "test")
		expect(result).toBeUndefined()
	})

	it("creates a backup directory with manifest", async () => {
		const filePath = join(filesDir, "config.json")
		await writeFile(filePath, '{"model": "opus"}')

		const backupDir = await createBackup([filePath], "Test backup")

		expect(backupDir).toBeDefined()
		expect(backupDir!).toContain("backups")

		// Manifest exists and is valid
		const manifestContent = await readFile(join(backupDir!, "manifest.json"), "utf-8")
		const manifest = JSON.parse(manifestContent)

		expect(manifest.description).toBe("Test backup")
		expect(manifest.version).toBe("0.1.0")
		expect(manifest.createdAt).toBeTruthy()
		expect(manifest.files).toHaveLength(1)
		expect(manifest.files[0].originalPath).toBe(filePath)
		expect(manifest.files[0].existedBefore).toBe(true)
	})

	it("backs up file content correctly", async () => {
		const filePath = join(filesDir, "settings.json")
		const originalContent = '{"theme": "dark"}'
		await writeFile(filePath, originalContent)

		const backupDir = await createBackup([filePath], "Content check")

		const manifestContent = await readFile(join(backupDir!, "manifest.json"), "utf-8")
		const manifest = JSON.parse(manifestContent)
		const entry = manifest.files[0]

		const backedUpContent = await readFile(join(backupDir!, "files", entry.backupFilename), "utf-8")
		expect(backedUpContent).toBe(originalContent)
	})

	it("marks non-existent files as existedBefore=false", async () => {
		const filePath = join(filesDir, "does-not-exist.json")

		const backupDir = await createBackup([filePath], "New file backup")

		const manifestContent = await readFile(join(backupDir!, "manifest.json"), "utf-8")
		const manifest = JSON.parse(manifestContent)

		expect(manifest.files[0].existedBefore).toBe(false)
	})

	it("handles multiple files", async () => {
		const file1 = join(filesDir, "a.json")
		const file2 = join(filesDir, "b.json")
		const file3 = join(filesDir, "c.json") // does not exist
		await writeFile(file1, "content-a")
		await writeFile(file2, "content-b")

		const backupDir = await createBackup([file1, file2, file3], "Multi-file backup")

		const manifestContent = await readFile(join(backupDir!, "manifest.json"), "utf-8")
		const manifest = JSON.parse(manifestContent)

		expect(manifest.files).toHaveLength(3)
		expect(manifest.files[0].existedBefore).toBe(true)
		expect(manifest.files[1].existedBefore).toBe(true)
		expect(manifest.files[2].existedBefore).toBe(false)
	})
})

// ─── listBackups ─────────────────────────────────────────────────────

describe("listBackups()", () => {
	it("returns empty array when no backups exist", async () => {
		const backups = await listBackups()
		expect(backups).toHaveLength(0)
	})

	it("lists backups sorted newest first", async () => {
		const file = join(filesDir, "test.json")
		await writeFile(file, "content")

		// Create two backups with a small delay to ensure different timestamps
		await createBackup([file], "First backup")
		// Wait 1.1 seconds to get a different timestamp (ISO format has second granularity)
		await new Promise((resolve) => setTimeout(resolve, 1100))
		await createBackup([file], "Second backup")

		const backups = await listBackups()

		expect(backups.length).toBeGreaterThanOrEqual(2)
		// Newest first
		expect(backups[0].manifest.description).toBe("Second backup")
		expect(backups[1].manifest.description).toBe("First backup")
	})

	it("returns BackupInfo with correct structure", async () => {
		const file = join(filesDir, "test.json")
		await writeFile(file, "content")

		await createBackup([file], "Structured backup")
		const backups = await listBackups()

		expect(backups.length).toBeGreaterThanOrEqual(1)
		const backup = backups[0]

		expect(backup.id).toBeTruthy()
		expect(backup.path).toContain(backup.id)
		expect(backup.manifest.description).toBe("Structured backup")
		expect(backup.manifest.files).toHaveLength(1)
	})
})

// ─── restore ─────────────────────────────────────────────────────────

describe("restore()", () => {
	it("throws when no backups exist", async () => {
		await expect(restore()).rejects.toThrow("No backups found")
	})

	it("restores files that existed before migration", async () => {
		const filePath = join(filesDir, "config.json")
		await writeFile(filePath, "original-content")

		// Create backup, then overwrite the file
		await createBackup([filePath], "Pre-migration")
		await writeFile(filePath, "migrated-content")

		// Verify file was overwritten
		expect(await readFile(filePath, "utf-8")).toBe("migrated-content")

		// Restore
		const result = await restore("latest")

		expect(result.restored).toContain(filePath)
		expect(result.errors).toHaveLength(0)

		// Content is back to original
		const content = await readFile(filePath, "utf-8")
		expect(content).toBe("original-content")
	})

	it("removes files that were newly created by migration", async () => {
		const existingFile = join(filesDir, "existing.json")
		const newFile = join(filesDir, "new-file.json")
		await writeFile(existingFile, "existing-content")

		// Create backup with both files (newFile doesn't exist yet)
		await createBackup([existingFile, newFile], "Pre-migration")

		// Simulate migration creating the new file
		await writeFile(newFile, "new-content")

		// Restore
		const result = await restore("latest")

		expect(result.restored).toContain(existingFile)
		expect(result.removed).toContain(newFile)

		// New file should be gone
		const { access } = await import("node:fs/promises")
		await expect(access(newFile)).rejects.toThrow()
	})

	it("restores a specific backup by ID", async () => {
		const filePath = join(filesDir, "test.json")
		await writeFile(filePath, "v1")

		const _backupDir1 = await createBackup([filePath], "Backup 1")
		await writeFile(filePath, "v2")

		// Wait to get different timestamp
		await new Promise((resolve) => setTimeout(resolve, 1100))
		await createBackup([filePath], "Backup 2")
		await writeFile(filePath, "v3")

		// Get the first backup's ID
		const backups = await listBackups()
		const oldestBackup = backups[backups.length - 1]

		// Restore the first (oldest) backup specifically
		const result = await restore(oldestBackup.id)

		expect(result.restored).toContain(filePath)
		const content = await readFile(filePath, "utf-8")
		expect(content).toBe("v1")
	})

	it("throws for non-existent backup ID", async () => {
		const filePath = join(filesDir, "test.json")
		await writeFile(filePath, "content")
		await createBackup([filePath], "A backup")

		await expect(restore("non-existent-id")).rejects.toThrow('Backup "non-existent-id" not found')
	})
})

// ─── deleteBackup ────────────────────────────────────────────────────

describe("deleteBackup()", () => {
	it("deletes a backup by ID", async () => {
		const filePath = join(filesDir, "test.json")
		await writeFile(filePath, "content")

		await createBackup([filePath], "To be deleted")

		const backupsBefore = await listBackups()
		expect(backupsBefore.length).toBeGreaterThanOrEqual(1)

		await deleteBackup(backupsBefore[0].id)

		const backupsAfter = await listBackups()
		expect(backupsAfter.length).toBe(backupsBefore.length - 1)
	})

	it("throws for non-existent backup", async () => {
		await expect(deleteBackup("does-not-exist")).rejects.toThrow(
			'Backup "does-not-exist" not found',
		)
	})
})
