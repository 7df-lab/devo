import { eq, inArray, sql } from "drizzle-orm"
import type { getDb } from "./database"
import { automationRuns, automations } from "./schema"

export const INTERRUPTED_RUN_MESSAGE =
	"Devo stopped before this run completed. Its outcome is unknown; inspect the session and workspace before retrying."

/**
 * A new Desktop process cannot safely resume or replay a previous process's
 * work. Close stale records atomically and retain session/worktree evidence.
 * Only invoke before this process admits any new automation execution.
 */
export async function recoverInterruptedRuns(
	db: ReturnType<typeof getDb>,
	now = Date.now(),
): Promise<number> {
	return db.transaction(async (tx) => {
		const interrupted = await tx
			.select()
			.from(automationRuns)
			.where(inArray(automationRuns.status, ["queued", "running"]))
		if (interrupted.length === 0) return 0

		await tx
			.update(automationRuns)
			.set({
				status: "failed",
				errorMessage: INTERRUPTED_RUN_MESSAGE,
				completedAt: now,
				updatedAt: now,
			})
			.where(inArray(automationRuns.status, ["queued", "running"]))
			.run()

		const counts = new Map<string, number>()
		for (const run of interrupted)
			counts.set(run.automationId, (counts.get(run.automationId) ?? 0) + 1)
		for (const [id, count] of counts) {
			await tx
				.update(automations)
				.set({
					runCount: sql`${automations.runCount} + ${count}`,
					consecutiveFailures: sql`${automations.consecutiveFailures} + ${count}`,
					updatedAt: now,
				})
				.where(eq(automations.id, id))
				.run()
		}
		return interrupted.length
	})
}
