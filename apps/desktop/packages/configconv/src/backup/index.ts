/**
 * Backup and restore module.
 *
 * Creates timestamped snapshots of files before migration so users can
 * safely revert if something goes wrong.
 *
 * Backup structure:
 *   ~/.config/devo/backups/
 *     2026-02-11T12-00-00/
 *       manifest.json        -- metadata + file list
 *       files/
 *         0001.dat           -- file contents, keyed by manifest index
 *         0002.dat
 *         ...
 */
import { readdir } from "node:fs/promises"
import { join } from "node:path"
import { ensureDir, exists, safeReadFile, safeReadJson, writeFileSafe } from "../utils/fs"
import { stringifyJson } from "../utils/json"
import * as paths from "../utils/paths"

// ─── Types ───────────────────────────────────────────────────────────

export interface BackupManifest {
	/** ISO timestamp of when the backup was created */
	createdAt: string
	/** cc2oc version that created this backup */
	version: string
	/** Human-readable description */
	description: string
	/** Files that were backed up */
	files: BackupFileEntry[]
}

export interface BackupFileEntry {
	/** Original absolute path of the file */
	originalPath: string
	/** Filename inside the backup's files/ directory */
	backupFilename: string
	/** Whether the file existed before migration (false = newly created by migration) */
	existedBefore: boolean
	/** SHA-256 hash of the content (for integrity checks) */
	hash?: string
}

export interface BackupInfo {
	/** Backup directory name (timestamp) */
	id: string
	/** Full path to backup directory */
	path: string
	/** Parsed manifest */
	manifest: BackupManifest
}

export interface RestoreResult {
	/** Files that were restored to their original location */
	restored: string[]
	/** Files that were removed (newly created by migration, not in backup) */
	removed: string[]
	/** Files that failed to restore */
	errors: Array<{ path: string; error: string }>
}

// ─── Backup ──────────────────────────────────────────────────────────

/**
 * Create a backup of all files that will be affected by a migration.
 *
 * @param targetPaths - All file paths that the writer will touch
 * @param description - Human-readable description of why this backup was made
 * @returns The backup directory path, or undefined if no files needed backing up
 */
export async function createBackup(
	targetPaths: string[],
	description = "Pre-migration backup",
): Promise<string | undefined> {
	if (targetPaths.length === 0) return undefined

	const backupsDir = paths.ocBackupsDir()
	const timestamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)
	const backupDir = join(backupsDir, timestamp)
	const filesDir = join(backupDir, "files")

	await ensureDir(filesDir)

	const manifest: BackupManifest = {
		createdAt: new Date().toISOString(),
		version: "0.1.0",
		description,
		files: [],
	}

	let fileIndex = 0

	for (const targetPath of targetPaths) {
		fileIndex++
		const backupFilename = `${fileIndex.toString().padStart(4, "0")}.dat`
		const fileExists = await exists(targetPath)

		if (fileExists) {
			// Back up existing file content
			const content = await safeReadFile(targetPath)
			if (content !== undefined) {
				await writeFileSafe(join(filesDir, backupFilename), content)
			}
		}

		manifest.files.push({
			originalPath: targetPath,
			backupFilename,
			existedBefore: fileExists,
		})
	}

	// Write manifest
	await writeFileSafe(join(backupDir, "manifest.json"), stringifyJson(manifest))

	return backupDir
}

// ─── List ────────────────────────────────────────────────────────────

/**
 * List all available backups, sorted newest first.
 */
export async function listBackups(): Promise<BackupInfo[]> {
	const backupsDir = paths.ocBackupsDir()
	if (!(await exists(backupsDir))) return []

	const entries = await readdir(backupsDir)
	const backups: BackupInfo[] = []

	for (const entry of entries) {
		const backupDir = join(backupsDir, entry)
		const manifestPath = join(backupDir, "manifest.json")
		const manifest = await safeReadJson<BackupManifest>(manifestPath)

		if (manifest) {
			backups.push({
				id: entry,
				path: backupDir,
				manifest,
			})
		}
	}

	// Sort newest first
	backups.sort((a, b) => b.manifest.createdAt.localeCompare(a.manifest.createdAt))

	return backups
}

// ─── Restore ─────────────────────────────────────────────────────────

/**
 * Restore files from a backup.
 *
 * - Files that existed before migration are restored to their original content
 * - Files that were newly created by migration are deleted
 *
 * @param backupId - Backup directory name (timestamp), or "latest" for the most recent
 * @returns Summary of what was restored
 */
export async function restore(backupId?: string): Promise<RestoreResult> {
	const result: RestoreResult = {
		restored: [],
		removed: [],
		errors: [],
	}

	// Find the backup
	const backups = await listBackups()
	if (backups.length === 0) {
		throw new Error("No backups found. Run `configconv migrate` first to create a backup.")
	}

	let backup: BackupInfo | undefined
	if (!backupId || backupId === "latest") {
		backup = backups[0]
	} else {
		backup = backups.find((b) => b.id === backupId)
	}

	if (!backup) {
		throw new Error(
			`Backup "${backupId}" not found. Available: ${backups.map((b) => b.id).join(", ")}`,
		)
	}

	const filesDir = join(backup.path, "files")

	for (const entry of backup.manifest.files) {
		try {
			if (entry.existedBefore) {
				// Restore original content
				const backupContent = await safeReadFile(join(filesDir, entry.backupFilename))
				if (backupContent !== undefined) {
					await writeFileSafe(entry.originalPath, backupContent)
					result.restored.push(entry.originalPath)
				} else {
					result.errors.push({
						path: entry.originalPath,
						error: "Backup file content not found",
					})
				}
			} else {
				// File was newly created by migration -- remove it
				const { unlink } = await import("node:fs/promises")
				if (await exists(entry.originalPath)) {
					await unlink(entry.originalPath)
					result.removed.push(entry.originalPath)
				}
			}
		} catch (err) {
			result.errors.push({
				path: entry.originalPath,
				error: err instanceof Error ? err.message : String(err),
			})
		}
	}

	return result
}

// ─── Cleanup ─────────────────────────────────────────────────────────

/**
 * Delete a specific backup.
 */
export async function deleteBackup(backupId: string): Promise<void> {
	const backupsDir = paths.ocBackupsDir()
	const backupDir = join(backupsDir, backupId)

	if (!(await exists(backupDir))) {
		throw new Error(`Backup "${backupId}" not found.`)
	}

	const { rm } = await import("node:fs/promises")
	await rm(backupDir, { recursive: true })
}
