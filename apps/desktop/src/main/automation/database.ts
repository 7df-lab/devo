import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createClient } from "@libsql/client"
import { drizzle } from "drizzle-orm/libsql"
import { migrate } from "drizzle-orm/libsql/migrator"
import { createLogger } from "../logger"
import { getDataDir } from "./paths"
import * as schema from "./schema"

const log = createLogger("automation-db")

// ESM equivalent for __dirname
const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

let client: ReturnType<typeof createClient> | null = null
let db: ReturnType<typeof drizzle<typeof schema>> | null = null

/**
 * Initializes and returns the Drizzle database instance using LibSQL.
 * Creates the DB file if it doesn't exist and runs all pending migrations.
 */
let initPromise: Promise<void> | null = null

export function getDb() {
	if (db) return db

	const dataDir = getDataDir()
	fs.mkdirSync(dataDir, { recursive: true })
	const dbPath = path.join(dataDir, "devo.db")
	log.info("Initializing automation database", { path: dbPath })

	// LibSQL uses a URL-style path for local files
	client = createClient({
		url: `file:${dbPath}`,
	})

	db = drizzle({ client, schema })

	// Run migrations asynchronously -- callers should await ensureDb() for migration safety
	const migrationsPath = path.join(__dirname, "drizzle")
	initPromise = migrate(db, { migrationsFolder: migrationsPath })
		.then(() => {
			log.info("Database migrations complete")
		})
		.catch((err) => {
			log.error("Database migration failed", err)
			throw err
		})

	return db
}

/** Wait for database migrations to complete. Call once at startup. */
export async function ensureDb(): Promise<ReturnType<typeof drizzle<typeof schema>>> {
	const instance = getDb()
	if (initPromise) {
		await initPromise
		initPromise = null
	}
	return instance
}

/**
 * Closes the database connection.
 */
export function closeDb() {
	if (client) {
		client.close()
		client = null
		db = null
		log.info("Database connection closed")
	}
}
