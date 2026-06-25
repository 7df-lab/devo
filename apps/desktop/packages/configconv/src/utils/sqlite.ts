/**
 * Platform-adaptive SQLite abstraction.
 *
 * Selects the best available SQLite driver at runtime:
 * 1. bun:sqlite   -- when running in Bun (CLI, tests)
 * 2. node:sqlite  -- when running in Node 22+ / Electron 33+ (built-in, no native addon)
 * 3. better-sqlite3 -- legacy fallback for older Node.js without built-in SQLite
 *
 * All three libraries expose a similar synchronous API, so we wrap them with
 * a minimal interface that covers our read-only use case.
 */

// ============================================================
// Minimal interface (subset of all three drivers)
// ============================================================

export interface SqliteDatabase {
	/** Prepare a SQL statement */
	prepare(sql: string): SqliteStatement
	/** Close the database */
	close(): void
}

export interface SqliteStatement {
	/** Execute and return the first matching row */
	get(...params: unknown[]): Record<string, unknown> | undefined
	/** Execute and return all matching rows */
	all(...params: unknown[]): Record<string, unknown>[]
}

// ============================================================
// Runtime detection and factory
// ============================================================

/** Detect whether we're running in Bun */
const isBun = typeof (globalThis as Record<string, unknown>).Bun !== "undefined"

/**
 * Open a SQLite database in read-only mode.
 *
 * Automatically selects the best available SQLite driver:
 * - Bun: uses bun:sqlite (built-in, no native addon)
 * - Node 22+ / Electron 33+: uses node:sqlite (built-in, no native addon)
 * - Older Node.js: uses better-sqlite3 (native addon, requires ABI match)
 */
export function openDatabase(path: string): SqliteDatabase {
	if (isBun) {
		return openBunDatabase(path)
	}

	// Try node:sqlite first (available in Node 22+ / Electron 33+)
	const nodeDb = tryOpenNodeSqliteDatabase(path)
	if (nodeDb) {
		return nodeDb
	}

	// Fall back to better-sqlite3 for older Node.js
	return openBetterSqliteDatabase(path)
}

// ============================================================
// Bun driver
// ============================================================

function openBunDatabase(path: string): SqliteDatabase {
	// Dynamic import to avoid parse errors in non-Bun runtimes.
	// bun:sqlite is a Bun builtin, so require() works synchronously.
	// biome-ignore lint/suspicious/noExplicitAny: bun:sqlite is not typed in Node
	const BunDatabase = (require as any)("bun:sqlite").Database
	const db = new BunDatabase(path, { readonly: true })

	return {
		prepare(sql: string): SqliteStatement {
			const stmt = db.prepare(sql)
			return {
				get(...params: unknown[]) {
					const row = stmt.get(...params)
					if (!row) return undefined
					// bun:sqlite returns plain objects, values may be Buffer or string
					return normalizeRow(row as Record<string, unknown>)
				},
				all(...params: unknown[]) {
					const rows = stmt.all(...params) as Record<string, unknown>[]
					return rows.map(normalizeRow)
				},
			}
		},
		close() {
			db.close()
		},
	}
}

// ============================================================
// node:sqlite driver (Node 22+ / Electron 33+)
// ============================================================

function tryOpenNodeSqliteDatabase(path: string): SqliteDatabase | null {
	try {
		// node:sqlite is experimental in Node 22+ and stable in Electron 33+.
		// It may not exist in older runtimes, so we catch the require error.
		// biome-ignore lint/suspicious/noExplicitAny: node:sqlite may not exist
		const { DatabaseSync } = (require as any)("node:sqlite")
		const db = new DatabaseSync(path, { readOnly: true })

		return {
			prepare(sql: string): SqliteStatement {
				const stmt = db.prepare(sql)
				return {
					get(...params: unknown[]) {
						const row = stmt.get(...params)
						if (!row) return undefined
						return normalizeRow(row as Record<string, unknown>)
					},
					all(...params: unknown[]) {
						const rows = stmt.all(...params) as Record<string, unknown>[]
						return rows.map(normalizeRow)
					},
				}
			},
			close() {
				db.close()
			},
		}
	} catch {
		// node:sqlite not available in this runtime
		return null
	}
}

// ============================================================
// better-sqlite3 driver (legacy fallback)
// ============================================================

function openBetterSqliteDatabase(path: string): SqliteDatabase {
	// Dynamic require to avoid bundler issues when better-sqlite3 isn't installed
	// biome-ignore lint/suspicious/noExplicitAny: better-sqlite3 may not be available
	const BetterDatabase = (require as any)("better-sqlite3")
	const db = new BetterDatabase(path, { readonly: true })

	return {
		prepare(sql: string): SqliteStatement {
			const stmt = db.prepare(sql)
			return {
				get(...params: unknown[]) {
					const row = stmt.get(...params) as Record<string, unknown> | undefined
					if (!row) return undefined
					return normalizeRow(row)
				},
				all(...params: unknown[]) {
					const rows = stmt.all(...params) as Record<string, unknown>[]
					return rows.map(normalizeRow)
				},
			}
		},
		close() {
			db.close()
		},
	}
}

// ============================================================
// Shared helpers
// ============================================================

/**
 * Normalize a row's values. Drivers may return Buffer/Uint8Array for BLOB columns;
 * we convert them to strings since we only read JSON text data from state.vscdb.
 */
function normalizeRow(row: Record<string, unknown>): Record<string, unknown> {
	const result: Record<string, unknown> = {}
	for (const [key, value] of Object.entries(row)) {
		if (value instanceof Uint8Array || (typeof Buffer !== "undefined" && Buffer.isBuffer(value))) {
			result[key] = new TextDecoder().decode(value as Uint8Array)
		} else {
			result[key] = value
		}
	}
	return result
}
