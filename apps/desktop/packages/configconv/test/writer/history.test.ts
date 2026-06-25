/**
 * Tests for history session writer with deduplication.
 *
 * Tests both SQLite (default, v1.2.0+) and legacy flat-file modes.
 */

import { Database } from "bun:sqlite"
import { afterEach, describe, expect, test } from "bun:test"
import { mkdir, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { ConvertedSession } from "../../src/types/conversion-result"
import { writeHistorySessionsDetailed } from "../../src/writer/history"

// ============================================================
// Helpers
// ============================================================

function makeSession(overrides: { projectId?: string; sessionId?: string } = {}): ConvertedSession {
	const sessionId = overrides.sessionId ?? `ses_cursor_test${Date.now()}`
	const projectId = overrides.projectId ?? "proj_test_12345678"

	return {
		projectId,
		session: {
			id: sessionId,
			slug: "test-session",
			version: "imported",
			projectID: projectId,
			directory: "/tmp/test-project",
			title: "Test Session",
			time: { created: Date.now(), updated: Date.now() },
			summary: { additions: 0, deletions: 0, files: 0 },
		},
		messages: [
			{
				id: `msg_${sessionId}_0001`,
				sessionID: sessionId,
				role: "user",
				time: { created: Date.now(), updated: Date.now() },
				parts: [
					{
						id: `part_msg_${sessionId}_0001_001`,
						messageID: `msg_${sessionId}_0001`,
						type: "text",
						content: "Hello",
					},
				],
			},
		],
	}
}

function tempDir(): string {
	return join(tmpdir(), `devo-history-test-${Date.now()}-${Math.random().toString(36).slice(2)}`)
}

function tempDbPath(): string {
	return join(
		tmpdir(),
		`devo-history-test-${Date.now()}-${Math.random().toString(36).slice(2)}.db`,
	)
}

// ============================================================
// SQLite mode tests (default, v1.2.0+)
// ============================================================

describe("writeHistorySessionsDetailed (sqlite)", () => {
	const tempPaths: string[] = []

	afterEach(async () => {
		for (const p of tempPaths) {
			try {
				await rm(p, { recursive: true })
			} catch {}
			// Also clean up WAL/SHM files
			try {
				await rm(`${p}-wal`)
			} catch {}
			try {
				await rm(`${p}-shm`)
			} catch {}
		}
		tempPaths.length = 0
	})

	const createTestDb = () => {
		const dbPath = tempDbPath()
		tempPaths.push(dbPath)
		return dbPath
	}

	test("writes sessions to SQLite database", async () => {
		const dbPath = createTestDb()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_001" })]
		const result = await writeHistorySessionsDetailed(sessions, {
			storageDir: dbPath,
			mode: "sqlite",
		})

		expect(result.filesWritten.length).toBeGreaterThan(0)
		expect(result.duplicatesSkipped).toHaveLength(0)
		expect(result.totalProcessed).toBe(1)

		// Should have written project, session, message, and part records
		const projectRecords = result.filesWritten.filter((f) => f.startsWith("project:"))
		const sessionRecords = result.filesWritten.filter((f) => f.startsWith("session:"))
		const messageRecords = result.filesWritten.filter((f) => f.startsWith("message:"))
		const partRecords = result.filesWritten.filter((f) => f.startsWith("part:"))
		expect(projectRecords).toHaveLength(1)
		expect(sessionRecords).toHaveLength(1)
		expect(messageRecords).toHaveLength(1)
		expect(partRecords).toHaveLength(1)

		// Verify data is actually in the database
		const db = new Database(dbPath, { readonly: true })
		const session = db.prepare("SELECT * FROM session WHERE id = ?").get("ses_cursor_001") as
			| Record<string, unknown>
			| undefined
		expect(session).toBeDefined()
		expect(session!.title).toBe("Test Session")
		expect(session!.project_id).toBe("proj_abc")

		const messages = db.prepare("SELECT * FROM message WHERE session_id = ?").all("ses_cursor_001")
		expect(messages).toHaveLength(1)

		const parts = db.prepare("SELECT * FROM part WHERE session_id = ?").all("ses_cursor_001")
		expect(parts).toHaveLength(1)

		db.close()
	})

	test("stores message data as JSON in data column", async () => {
		const dbPath = createTestDb()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_json_test" })]
		await writeHistorySessionsDetailed(sessions, { storageDir: dbPath, mode: "sqlite" })

		const db = new Database(dbPath, { readonly: true })
		const msg = db.prepare("SELECT data FROM message LIMIT 1").get() as Record<string, unknown>
		const data = JSON.parse(msg.data as string)
		expect(data.role).toBe("user")
		expect(data.time).toBeDefined()
		expect(data.time.created).toBeGreaterThan(0)

		const part = db.prepare("SELECT data FROM part LIMIT 1").get() as Record<string, unknown>
		const partData = JSON.parse(part.data as string)
		expect(partData.type).toBe("text")
		expect(partData.text).toBe("Hello")
		expect(partData.synthetic).toBe(false)

		db.close()
	})

	test("skips duplicate sessions on second write", async () => {
		const dbPath = createTestDb()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_dedup1" })]

		// First write
		const result1 = await writeHistorySessionsDetailed(sessions, {
			storageDir: dbPath,
			mode: "sqlite",
		})
		expect(result1.filesWritten.length).toBeGreaterThan(0)
		expect(result1.duplicatesSkipped).toHaveLength(0)

		// Second write with same session ID
		const result2 = await writeHistorySessionsDetailed(sessions, {
			storageDir: dbPath,
			mode: "sqlite",
		})
		expect(result2.filesWritten).toHaveLength(0)
		expect(result2.duplicatesSkipped).toHaveLength(1)
		expect(result2.duplicatesSkipped[0]).toBe("ses_cursor_dedup1")
		expect(result2.totalProcessed).toBe(1)
	})

	test("writes new sessions and skips existing ones", async () => {
		const dbPath = createTestDb()

		const session1 = makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_existing" })
		const session2 = makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_new" })

		// Write first session
		await writeHistorySessionsDetailed([session1], { storageDir: dbPath, mode: "sqlite" })

		// Write both sessions - first should be skipped, second should be written
		const result = await writeHistorySessionsDetailed([session1, session2], {
			storageDir: dbPath,
			mode: "sqlite",
		})
		expect(result.duplicatesSkipped).toHaveLength(1)
		expect(result.duplicatesSkipped[0]).toBe("ses_cursor_existing")
		expect(result.filesWritten.length).toBeGreaterThan(0)
		expect(result.totalProcessed).toBe(2)
	})

	test("calls progress callback during write", async () => {
		const dbPath = createTestDb()

		const sessions = [
			makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_prog1" }),
			makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_prog2" }),
		]

		const progressCalls: string[] = []
		await writeHistorySessionsDetailed(sessions, {
			storageDir: dbPath,
			mode: "sqlite",
			onProgress: (progress) => {
				progressCalls.push(progress.phase)
			},
		})

		expect(progressCalls).toContain("dedup-check")
		expect(progressCalls).toContain("writing")
		expect(progressCalls).toContain("complete")
	})

	test("does not recreate existing project records", async () => {
		const dbPath = createTestDb()

		const sessions1 = [makeSession({ projectId: "proj_same", sessionId: "ses_cursor_p1" })]

		// First write creates project record
		const result1 = await writeHistorySessionsDetailed(sessions1, {
			storageDir: dbPath,
			mode: "sqlite",
		})
		const projectRecords1 = result1.filesWritten.filter((f) => f.startsWith("project:"))
		expect(projectRecords1).toHaveLength(1)

		// Second write with a NEW session but same project
		const sessions2 = [makeSession({ projectId: "proj_same", sessionId: "ses_cursor_p2" })]
		const result2 = await writeHistorySessionsDetailed(sessions2, {
			storageDir: dbPath,
			mode: "sqlite",
		})
		const projectRecords2 = result2.filesWritten.filter((f) => f.startsWith("project:"))
		expect(projectRecords2).toHaveLength(0) // Project record already exists
	})

	test("handles empty sessions array", async () => {
		const dbPath = createTestDb()

		const result = await writeHistorySessionsDetailed([], {
			storageDir: dbPath,
			mode: "sqlite",
		})
		expect(result.filesWritten).toHaveLength(0)
		expect(result.duplicatesSkipped).toHaveLength(0)
		expect(result.totalProcessed).toBe(0)
	})

	test("reports progress with dedup count", async () => {
		const dbPath = createTestDb()

		// Pre-write one session
		await writeHistorySessionsDetailed(
			[makeSession({ projectId: "proj_abc", sessionId: "ses_existing" })],
			{ storageDir: dbPath, mode: "sqlite" },
		)

		// Now write with one existing and one new
		const sessions = [
			makeSession({ projectId: "proj_abc", sessionId: "ses_existing" }),
			makeSession({ projectId: "proj_abc", sessionId: "ses_brand_new" }),
		]

		let lastProgress: { duplicatesSkipped: number; phase: string } | null = null
		await writeHistorySessionsDetailed(sessions, {
			storageDir: dbPath,
			mode: "sqlite",
			onProgress: (p) => {
				lastProgress = p
			},
		})

		expect(lastProgress).not.toBeNull()
		expect(lastProgress!.phase).toBe("complete")
		expect(lastProgress!.duplicatesSkipped).toBe(1)
	})

	test("defaults to sqlite mode when no mode specified", async () => {
		const dbPath = createTestDb()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_default_mode" })]
		const result = await writeHistorySessionsDetailed(sessions, { storageDir: dbPath })

		// Should write records (not files), indicating SQLite mode was used
		const sessionRecords = result.filesWritten.filter((f) => f.startsWith("session:"))
		expect(sessionRecords).toHaveLength(1)
	})
})

// ============================================================
// Legacy flat-file mode tests (pre-v1.2.0)
// ============================================================

describe("writeHistorySessionsDetailed (legacy)", () => {
	const tempDirs: string[] = []

	afterEach(async () => {
		for (const dir of tempDirs) {
			try {
				await rm(dir, { recursive: true })
			} catch {}
		}
		tempDirs.length = 0
	})

	const createTestDir = async () => {
		const dir = tempDir()
		tempDirs.push(dir)
		await mkdir(dir, { recursive: true })
		return dir
	}

	test("writes sessions to storage directory", async () => {
		const storageDir = await createTestDir()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_001" })]
		const result = await writeHistorySessionsDetailed(sessions, {
			storageDir,
			mode: "legacy",
		})

		expect(result.filesWritten.length).toBeGreaterThan(0)
		expect(result.duplicatesSkipped).toHaveLength(0)
		expect(result.totalProcessed).toBe(1)

		// Should have written project, session, message, and part files
		const projectFiles = result.filesWritten.filter((f) => f.includes("/project/"))
		const sessionFiles = result.filesWritten.filter((f) => f.includes("/session/"))
		const messageFiles = result.filesWritten.filter((f) => f.includes("/message/"))
		const partFiles = result.filesWritten.filter((f) => f.includes("/part/"))
		expect(projectFiles).toHaveLength(1)
		expect(sessionFiles).toHaveLength(1)
		expect(messageFiles).toHaveLength(1)
		expect(partFiles).toHaveLength(1)
	})

	test("skips duplicate sessions on second write", async () => {
		const storageDir = await createTestDir()

		const sessions = [makeSession({ projectId: "proj_abc", sessionId: "ses_cursor_dedup1" })]

		// First write
		const result1 = await writeHistorySessionsDetailed(sessions, {
			storageDir,
			mode: "legacy",
		})
		expect(result1.filesWritten.length).toBeGreaterThan(0)
		expect(result1.duplicatesSkipped).toHaveLength(0)

		// Second write with same session ID
		const result2 = await writeHistorySessionsDetailed(sessions, {
			storageDir,
			mode: "legacy",
		})
		expect(result2.filesWritten).toHaveLength(0)
		expect(result2.duplicatesSkipped).toHaveLength(1)
		expect(result2.duplicatesSkipped[0]).toBe("ses_cursor_dedup1")
		expect(result2.totalProcessed).toBe(1)
	})

	test("handles empty sessions array", async () => {
		const storageDir = await createTestDir()

		const result = await writeHistorySessionsDetailed([], {
			storageDir,
			mode: "legacy",
		})
		expect(result.filesWritten).toHaveLength(0)
		expect(result.duplicatesSkipped).toHaveLength(0)
		expect(result.totalProcessed).toBe(0)
	})
})
