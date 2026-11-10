import { expect, mock, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createClient } from "@libsql/client"
import { drizzle } from "drizzle-orm/libsql"
import { migrate } from "drizzle-orm/libsql/migrator"
import * as schema from "./schema"

test("startup is single-flight, API reads wait for recovery, and quit rejects new work", async () => {
	const dir = mkdtempSync(path.join(os.tmpdir(), "devo-lifecycle-"))
	const client = createClient({ url: `file:${path.join(dir, "test.db")}` })
	try {
		const db = drizzle({ client, schema })
		await migrate(db, {
			migrationsFolder: fileURLToPath(new URL("../../../drizzle", import.meta.url)),
		})
		await db.insert(schema.automations).values({ id: "a", createdAt: 1, updatedAt: 1 }).run()
		await db
			.insert(schema.automationRuns)
			.values({
				id: "orphan",
				automationId: "a",
				workspace: "",
				status: "running",
				createdAt: 1,
				updatedAt: 1,
			})
			.run()
		let release!: () => void
		const ready = new Promise<void>((resolve) => {
			release = resolve
		})
		let initializations = 0
		mock.module("./database", () => ({
			ensureDb: async () => {
				initializations++
				await ready
				return db
			},
			getDb: () => db,
			closeDb: () => client.close(),
		}))
		mock.module("./registry", () => ({
			listConfigs: () => [],
			readConfig: () => null,
			createConfig: () => "a",
			updateConfig: () => false,
			deleteConfig: () => false,
		}))
		mock.module("./executor", () => ({
			executeRun: async () => {
				throw new Error("Must not replay")
			},
		}))
		mock.module("electron", () => ({ BrowserWindow: { getAllWindows: () => [] } }))
		const { initAutomations, listRuns, runNow, shutdownAutomations } = await import("./index")
		const startup = initAutomations()
		let readCompleted = false
		const read = listRuns().then((runs) => {
			readCompleted = true
			return runs
		})
		await Promise.resolve()
		expect(initializations).toBe(1)
		expect(readCompleted).toBe(false)
		release()
		await startup
		const runs = await read
		expect(runs.map(({ id, status }) => ({ id, status }))).toEqual([
			{ id: "orphan", status: "failed" },
		])
		shutdownAutomations()
		await expect(runNow("a")).rejects.toThrow("shutting down")
		await expect(listRuns()).rejects.toThrow("shutting down")
	} finally {
		client.close()
		rmSync(dir, { recursive: true, force: true })
	}
})
