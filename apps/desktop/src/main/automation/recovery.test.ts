import { describe, expect, test } from "bun:test"
import { mkdtempSync, rmSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createClient } from "@libsql/client"
import { drizzle } from "drizzle-orm/libsql"
import { migrate } from "drizzle-orm/libsql/migrator"
import { INTERRUPTED_RUN_MESSAGE, recoverInterruptedRuns } from "./recovery"
import * as schema from "./schema"

describe("automation restart recovery", () => {
	test("closes interrupted records once, preserves evidence and never replays work", async () => {
		const dir = mkdtempSync(path.join(os.tmpdir(), "devo-recovery-"))
		const client = createClient({ url: `file:${path.join(dir, "test.db")}` })
		try {
			const db = drizzle({ client, schema })
			await migrate(db, {
				migrationsFolder: fileURLToPath(new URL("../../../drizzle", import.meta.url)),
			})
			await db.insert(schema.automations).values({ id: "a", createdAt: 1, updatedAt: 1 }).run()
			await db
				.insert(schema.automationRuns)
				.values([
					{
						id: "running",
						automationId: "a",
						workspace: "project",
						status: "running",
						sessionId: "session",
						worktreePath: "worktree",
						createdAt: 1,
						updatedAt: 1,
					},
					{
						id: "queued",
						automationId: "a",
						workspace: "project",
						status: "queued",
						createdAt: 1,
						updatedAt: 1,
					},
					{
						id: "done",
						automationId: "a",
						workspace: "project",
						status: "pending_review",
						completedAt: 2,
						createdAt: 1,
						updatedAt: 2,
					},
				])
				.run()
			const before = await db.select().from(schema.automationRuns).orderBy(schema.automationRuns.id)
			expect(await recoverInterruptedRuns(db, 100)).toBe(2)
			expect(
				await db.select().from(schema.automationRuns).orderBy(schema.automationRuns.id),
			).toEqual(
				before.map((run) =>
					run.status === "pending_review"
						? run
						: {
								...run,
								status: "failed",
								errorMessage: INTERRUPTED_RUN_MESSAGE,
								completedAt: 100,
								updatedAt: 100,
							},
				),
			)
			expect(await recoverInterruptedRuns(db, 200)).toBe(0)
			expect(await db.select().from(schema.automations)).toEqual([
				{
					id: "a",
					nextRunAt: null,
					lastRunAt: null,
					runCount: 2,
					consecutiveFailures: 2,
					createdAt: 1,
					updatedAt: 100,
				},
			])
		} finally {
			client.close()
			rmSync(dir, { recursive: true, force: true })
		}
	})
})
