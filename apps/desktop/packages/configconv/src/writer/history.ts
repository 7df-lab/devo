/**
 * History session writer.
 *
 * Writes converted chat sessions (from any source: Claude Code, Cursor, etc.)
 * to the Devo storage format.
 *
 * As of Devo v1.2.0, storage uses a single SQLite database (devo.db)
 * instead of the previous flat-file layout. This writer supports both:
 *   - SQLite mode (default, v1.2.0+): writes to ~/.local/share/devo/devo.db
 *   - Legacy flat-file mode: writes JSON files to ~/.local/share/devo/storage/
 *
 * SQLite schema (from devo.db):
 *   project(id, worktree, vcs, name, ..., time_created, time_updated, sandboxes, commands)
 *   session(id, project_id, parent_id, slug, directory, title, version, ..., time_created, time_updated)
 *   message(id, session_id, time_created, time_updated, data TEXT)
 *   part(id, message_id, session_id, time_created, time_updated, data TEXT)
 *
 * Features:
 *   - Deduplication: checks for existing sessions before writing, skips duplicates
 *   - Progress reporting: optional callback for UI feedback
 *   - Atomic project writes: project records are only created if they don't exist
 */

import { join } from "node:path"
import type { ConvertedMessage, ConvertedSession } from "../types/conversion-result"
import { ensureDir, exists, safeReadDir, writeFileSafe } from "../utils/fs"
import { stringifyJson } from "../utils/json"
import * as paths from "../utils/paths"
import type { SqliteDatabase } from "../utils/sqlite"

// ============================================================
// Types
// ============================================================

export interface HistoryWriteResult {
	/** Files that were written to disk (legacy) or records inserted (sqlite) */
	filesWritten: string[]
	/** Session IDs that were skipped because they already exist */
	duplicatesSkipped: string[]
	/** Total sessions processed (written + skipped) */
	totalProcessed: number
}

export interface HistoryWriteProgress {
	/** Current phase */
	phase: "dedup-check" | "writing" | "complete"
	/** 1-based index of current session being processed */
	sessionIndex: number
	/** Total sessions to process */
	sessionCount: number
	/** Number of duplicates skipped so far */
	duplicatesSkipped: number
}

// ============================================================
// Public API
// ============================================================

export interface HistoryWriteOptions {
	/** Optional callback for progress reporting */
	onProgress?: (progress: HistoryWriteProgress) => void
	/**
	 * Override the storage path.
	 * In sqlite mode, this should be the path to the database file.
	 * In legacy mode, this should be the storage directory.
	 */
	storageDir?: string
	/**
	 * Storage mode: "sqlite" (default, v1.2.0+) or "legacy" (flat-file, pre-v1.2.0).
	 * @default "sqlite"
	 */
	mode?: "sqlite" | "legacy"
}

/**
 * Write converted sessions to Devo storage with deduplication.
 *
 * Checks for existing sessions and skips duplicates.
 * Returns both files written and duplicates skipped.
 *
 * @param sessions - Converted sessions from any source
 * @param onProgressOrOptions - Optional callback or options object
 * @returns Array of file paths that were written (or record descriptions for sqlite)
 */
export async function writeHistorySessions(
	sessions: ConvertedSession[],
	onProgressOrOptions?: ((progress: HistoryWriteProgress) => void) | HistoryWriteOptions,
): Promise<string[]> {
	const options =
		typeof onProgressOrOptions === "function"
			? { onProgress: onProgressOrOptions }
			: (onProgressOrOptions ?? {})
	const result = await writeHistorySessionsDetailed(sessions, options)
	return result.filesWritten
}

/**
 * Write converted sessions with detailed result including dedup information.
 */
export async function writeHistorySessionsDetailed(
	sessions: ConvertedSession[],
	options: HistoryWriteOptions = {},
): Promise<HistoryWriteResult> {
	const mode = options.mode ?? "sqlite"

	if (mode === "legacy") {
		return writeLegacyHistorySessions(sessions, options)
	}

	return writeSqliteHistorySessions(sessions, options)
}

// ============================================================
// SQLite writer (v1.2.0+)
// ============================================================

async function writeSqliteHistorySessions(
	sessions: ConvertedSession[],
	options: HistoryWriteOptions,
): Promise<HistoryWriteResult> {
	const { onProgress, storageDir: dbPathOverride } = options
	const dbPath = dbPathOverride ?? paths.ocDatabasePath()
	const recordsWritten: string[] = []
	const duplicatesSkipped: string[] = []

	// --- Deduplication phase ---
	onProgress?.({
		phase: "dedup-check",
		sessionIndex: 0,
		sessionCount: sessions.length,
		duplicatesSkipped: 0,
	})

	const db = openWritableDatabase(dbPath)

	try {
		const existingSessionIds = discoverExistingSessionIdsSqlite(db)

		// --- Collect unique projects and sessions to write ---
		const projects = new Map<string, { id: string; directory: string }>()
		const sessionsToWrite: ConvertedSession[] = []

		for (const session of sessions) {
			if (existingSessionIds.has(session.session.id)) {
				duplicatesSkipped.push(session.session.id)
				continue
			}

			sessionsToWrite.push(session)
			if (!projects.has(session.projectId)) {
				projects.set(session.projectId, {
					id: session.projectId,
					directory: session.session.directory,
				})
			}
		}

		// --- Write project records ---
		for (const [projectId, project] of projects) {
			const existing = db.prepare("SELECT id FROM project WHERE id = ?").get(projectId) as
				| Record<string, unknown>
				| undefined
			if (existing) continue

			const now = Date.now()
			db.prepare(
				`INSERT INTO project (id, worktree, vcs, sandboxes, time_created, time_updated)
				 VALUES (?, ?, ?, ?, ?, ?)`,
			).get(projectId, project.directory, "git", "[]", now, now)
			recordsWritten.push(`project:${projectId}`)
		}

		// --- Write session, message, and part records ---
		let sessionIndex = 0
		for (const session of sessionsToWrite) {
			sessionIndex++

			onProgress?.({
				phase: "writing",
				sessionIndex,
				sessionCount: sessionsToWrite.length,
				duplicatesSkipped: duplicatesSkipped.length,
			})

			const now = Date.now()
			const sessionData = session.session

			// Session record
			db.prepare(
				`INSERT INTO session (id, project_id, slug, directory, title, version,
				 summary_additions, summary_deletions, summary_files,
				 time_created, time_updated)
				 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
			).get(
				sessionData.id,
				session.projectId,
				sessionData.slug,
				sessionData.directory,
				sessionData.title,
				sessionData.version ?? "imported",
				sessionData.summary?.additions ?? 0,
				sessionData.summary?.deletions ?? 0,
				sessionData.summary?.files ?? 0,
				sessionData.time.created,
				sessionData.time.updated,
			)
			recordsWritten.push(`session:${sessionData.id}`)

			// Message and part records
			for (const message of session.messages) {
				const messageJson = buildMessageData(message)
				db.prepare(
					`INSERT INTO message (id, session_id, time_created, time_updated, data)
					 VALUES (?, ?, ?, ?, ?)`,
				).get(message.id, message.sessionID, now, now, JSON.stringify(messageJson))
				recordsWritten.push(`message:${message.id}`)

				for (const part of message.parts) {
					const partJson = {
						type: part.type,
						text: part.content,
						synthetic: false,
						time: { start: 0, end: 0 },
					}
					db.prepare(
						`INSERT INTO part (id, message_id, session_id, time_created, time_updated, data)
						 VALUES (?, ?, ?, ?, ?, ?)`,
					).get(part.id, message.id, message.sessionID, now, now, JSON.stringify(partJson))
					recordsWritten.push(`part:${part.id}`)
				}
			}
		}

		onProgress?.({
			phase: "complete",
			sessionIndex: sessionsToWrite.length,
			sessionCount: sessionsToWrite.length,
			duplicatesSkipped: duplicatesSkipped.length,
		})
	} finally {
		db.close()
	}

	return {
		filesWritten: recordsWritten,
		duplicatesSkipped,
		totalProcessed: sessions.length,
	}
}

/**
 * Build the JSON data blob for a message record in the SQLite database.
 * The `data` column stores the message metadata (role, time, etc.) as JSON.
 */
function buildMessageData(message: ConvertedMessage): Record<string, unknown> {
	return {
		role: message.role,
		time: {
			created: message.time.created,
			...(message.role === "assistant" ? { completed: message.time.updated } : {}),
		},
	}
}

function discoverExistingSessionIdsSqlite(db: SqliteDatabase): Set<string> {
	const rows = db.prepare("SELECT id FROM session").all()
	return new Set(rows.map((r) => r.id as string))
}

// ============================================================
// SQLite database creation (writable mode)
// ============================================================

/**
 * Open (or create) a writable SQLite database.
 * Unlike the read-only openDatabase() from utils/sqlite, this opens in write mode
 * and initializes the schema if the database is new.
 */
function openWritableDatabase(dbPath: string): SqliteDatabase {
	const isBun = typeof (globalThis as Record<string, unknown>).Bun !== "undefined"

	if (isBun) {
		// biome-ignore lint/suspicious/noExplicitAny: bun:sqlite is not typed in Node
		const BunDatabase = (require as any)("bun:sqlite").Database
		const raw = new BunDatabase(dbPath)
		raw.exec(DEVO_SCHEMA)
		return wrapRawDb(raw)
	}

	try {
		// biome-ignore lint/suspicious/noExplicitAny: node:sqlite may not exist
		const { DatabaseSync } = (require as any)("node:sqlite")
		const raw = new DatabaseSync(dbPath)
		raw.exec(DEVO_SCHEMA)
		return wrapRawDb(raw)
	} catch {
		// biome-ignore lint/suspicious/noExplicitAny: better-sqlite3 may not be available
		const BetterDatabase = (require as any)("better-sqlite3")
		const raw = new BetterDatabase(dbPath)
		raw.exec(DEVO_SCHEMA)
		return wrapRawDb(raw)
	}
}

/**
 * Wrap a raw SQLite driver instance with our minimal SqliteDatabase interface.
 * Works with bun:sqlite, node:sqlite DatabaseSync, and better-sqlite3 since
 * they all share the same prepare/get/all/close API shape.
 */
// biome-ignore lint/suspicious/noExplicitAny: cross-driver wrapper
function wrapRawDb(raw: any): SqliteDatabase {
	return {
		prepare(sql: string) {
			const stmt = raw.prepare(sql)
			return {
				get(...params: unknown[]) {
					return stmt.get(...params) as Record<string, unknown> | undefined
				},
				all(...params: unknown[]) {
					return stmt.all(...params) as Record<string, unknown>[]
				},
			}
		},
		close() {
			raw.close()
		},
	}
}

const DEVO_SCHEMA = `
CREATE TABLE IF NOT EXISTS project (
	id TEXT PRIMARY KEY,
	worktree TEXT NOT NULL,
	vcs TEXT,
	name TEXT,
	icon_url TEXT,
	icon_color TEXT,
	time_created INTEGER NOT NULL,
	time_updated INTEGER NOT NULL,
	time_initialized INTEGER,
	sandboxes TEXT NOT NULL,
	commands TEXT
);

CREATE TABLE IF NOT EXISTS session (
	id TEXT PRIMARY KEY,
	project_id TEXT NOT NULL,
	parent_id TEXT,
	slug TEXT NOT NULL,
	directory TEXT NOT NULL,
	title TEXT NOT NULL,
	version TEXT NOT NULL,
	share_url TEXT,
	summary_additions INTEGER,
	summary_deletions INTEGER,
	summary_files INTEGER,
	summary_diffs TEXT,
	revert TEXT,
	permission TEXT,
	time_created INTEGER NOT NULL,
	time_updated INTEGER NOT NULL,
	time_compacting INTEGER,
	time_archived INTEGER,
	FOREIGN KEY (project_id) REFERENCES project(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS message (
	id TEXT PRIMARY KEY,
	session_id TEXT NOT NULL,
	time_created INTEGER NOT NULL,
	time_updated INTEGER NOT NULL,
	data TEXT NOT NULL,
	FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS part (
	id TEXT PRIMARY KEY,
	message_id TEXT NOT NULL,
	session_id TEXT NOT NULL,
	time_created INTEGER NOT NULL,
	time_updated INTEGER NOT NULL,
	data TEXT NOT NULL,
	FOREIGN KEY (message_id) REFERENCES message(id) ON DELETE CASCADE
);
`

// ============================================================
// Legacy flat-file writer (pre-v1.2.0)
// ============================================================

/**
 * Legacy flat-file writer for backward compatibility.
 *
 * Devo storage layout (pre-v1.2.0):
 *   ~/.local/share/devo/storage/
 *     project/<projectId>.json         - Project metadata
 *     session/<projectId>/<sessionId>.json - Session metadata
 *     message/<sessionId>/<messageId>.json - Message metadata
 *     part/<messageId>/<partId>.json       - Message parts (text, tools, reasoning)
 */
async function writeLegacyHistorySessions(
	sessions: ConvertedSession[],
	options: HistoryWriteOptions,
): Promise<HistoryWriteResult> {
	const { onProgress, storageDir: storageDirOverride } = options
	const storageDir = storageDirOverride ?? paths.ocStorageDir()
	const filesWritten: string[] = []
	const duplicatesSkipped: string[] = []

	// --- Deduplication: discover existing session IDs ---
	onProgress?.({
		phase: "dedup-check",
		sessionIndex: 0,
		sessionCount: sessions.length,
		duplicatesSkipped: 0,
	})

	const existingSessionIds = await discoverExistingSessionIdsLegacy(storageDir)

	// --- Collect unique projects ---
	const projects = new Map<string, { id: string; directory: string }>()
	const sessionsToWrite: ConvertedSession[] = []

	for (const session of sessions) {
		if (existingSessionIds.has(session.session.id)) {
			duplicatesSkipped.push(session.session.id)
			continue
		}

		sessionsToWrite.push(session)
		if (!projects.has(session.projectId)) {
			projects.set(session.projectId, {
				id: session.projectId,
				directory: session.session.directory,
			})
		}
	}

	// --- Write project files (skip if already exists) ---
	for (const [projectId, project] of projects) {
		const projectPath = join(storageDir, "project", `${projectId}.json`)
		if (await exists(projectPath)) continue

		const projectData = {
			id: projectId,
			worktree: project.directory,
			vcs: "git",
			sandboxes: [],
			time: {
				created: Date.now(),
				updated: Date.now(),
			},
		}
		await writeFileSafe(projectPath, stringifyJson(projectData))
		filesWritten.push(projectPath)
	}

	// --- Write session, message, and part files ---
	let sessionIndex = 0
	for (const session of sessionsToWrite) {
		sessionIndex++

		onProgress?.({
			phase: "writing",
			sessionIndex,
			sessionCount: sessionsToWrite.length,
			duplicatesSkipped: duplicatesSkipped.length,
		})

		// Session file
		const sessionDir = join(storageDir, "session", session.projectId)
		await ensureDir(sessionDir)
		const sessionPath = join(sessionDir, `${session.session.id}.json`)
		await writeFileSafe(sessionPath, stringifyJson(session.session))
		filesWritten.push(sessionPath)

		// Message and part files
		for (const message of session.messages) {
			// Message file
			const messageDir = join(storageDir, "message", session.session.id)
			await ensureDir(messageDir)
			const messagePath = join(messageDir, `${message.id}.json`)

			const messageData = {
				id: message.id,
				sessionID: message.sessionID,
				role: message.role,
				time: message.time,
			}
			await writeFileSafe(messagePath, stringifyJson(messageData))
			filesWritten.push(messagePath)

			// Part files
			for (const part of message.parts) {
				const partDir = join(storageDir, "part", message.id)
				await ensureDir(partDir)
				const partPath = join(partDir, `${part.id}.json`)

				const partData = {
					id: part.id,
					type: part.type,
					text: part.content,
					synthetic: false,
					time: { start: 0, end: 0 },
					messageID: part.messageID,
					sessionID: message.sessionID,
				}
				await writeFileSafe(partPath, stringifyJson(partData))
				filesWritten.push(partPath)
			}
		}
	}

	onProgress?.({
		phase: "complete",
		sessionIndex: sessionsToWrite.length,
		sessionCount: sessionsToWrite.length,
		duplicatesSkipped: duplicatesSkipped.length,
	})

	return {
		filesWritten,
		duplicatesSkipped,
		totalProcessed: sessions.length,
	}
}

/**
 * Discover all existing session IDs in the legacy flat-file storage directory.
 */
async function discoverExistingSessionIdsLegacy(storageDir: string): Promise<Set<string>> {
	const sessionBaseDir = join(storageDir, "session")
	const ids = new Set<string>()

	const projectDirs = await safeReadDir(sessionBaseDir)
	for (const projectDir of projectDirs) {
		const fullDir = join(sessionBaseDir, projectDir)
		const files = await safeReadDir(fullDir)
		for (const file of files) {
			if (file.endsWith(".json")) {
				ids.add(file.slice(0, -5))
			}
		}
	}

	return ids
}
