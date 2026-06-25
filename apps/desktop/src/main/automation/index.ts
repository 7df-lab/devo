/**
 * Automation manager -- top-level module that initializes and coordinates
 * the automation subsystem (database, registry, scheduler).
 *
 * Exports the public API consumed by IPC handlers.
 */

import crypto from "node:crypto"
import { eq, sql } from "drizzle-orm"
import { BrowserWindow } from "electron"
import { createLogger } from "../logger"
import { closeDb, ensureDb, getDb } from "./database"
import { executeRun } from "./executor"
import { createConfig, deleteConfig, listConfigs, readConfig, updateConfig } from "./registry"
import { addTask, getNextRunTime, previewSchedule, removeTask, stopAll } from "./scheduler"
import { automationRuns, automations } from "./schema"
import { Semaphore } from "./semaphore"
import type {
	Automation,
	AutomationRun,
	CreateAutomationInput,
	UpdateAutomationInput,
} from "./types"

const log = createLogger("automation")

const semaphore = new Semaphore(5)

// ============================================================
// Broadcast helper
// ============================================================

/** Notify all renderer windows that automation data has changed. */
function broadcastRunsUpdated(): void {
	for (const win of BrowserWindow.getAllWindows()) {
		win.webContents.send("automation:runs-updated")
	}
}

// ============================================================
// Initialization
// ============================================================

/** Initialize the automation subsystem. Call once at app startup. */
export async function initAutomations(): Promise<void> {
	log.info("Initializing automation subsystem")
	const db = await ensureDb()

	// Load all active automations and schedule them
	const configs = listConfigs()
	for (const config of configs) {
		if (config.status !== "active") continue

		// Ensure timing row exists in SQLite
		const existing = await db.select().from(automations).where(eq(automations.id, config.id)).get()
		if (!existing) {
			await db
				.insert(automations)
				.values({
					id: config.id,
					createdAt: Date.now(),
					updatedAt: Date.now(),
				})
				.run()
		}

		await scheduleAndPersistNextRun(config.id, config.schedule.rrule, config.schedule.timezone)
	}

	log.info(`Loaded ${configs.filter((c) => c.status === "active").length} active automations`)
}

/** Shut down the automation subsystem. Call on app quit. */
export function shutdownAutomations(): void {
	stopAll()
	closeDb()
	log.info("Automation subsystem shut down")
}

// ============================================================
// Scheduling helpers
// ============================================================

/**
 * Schedule an automation and persist its computed `nextRunAt` to the database.
 * Called during init, after CRUD changes, and after each run completes.
 */
async function scheduleAndPersistNextRun(
	id: string,
	rruleStr: string,
	timezone: string,
): Promise<void> {
	log.info("Scheduling and persisting next run", { id, rruleStr, timezone })
	const nextRunAt = await addTask(id, rruleStr, timezone, async () => {
		await executeAutomation(id)
	})

	// Persist the computed nextRunAt
	const db = getDb()
	await db
		.update(automations)
		.set({
			nextRunAt: nextRunAt ? nextRunAt.getTime() : null,
			updatedAt: Date.now(),
		})
		.where(eq(automations.id, id))
		.run()

	log.info("Next run persisted to DB", {
		id,
		nextRunAt: nextRunAt?.toISOString() ?? "none",
	})
}

// ============================================================
// CRUD operations
// ============================================================

export async function listAutomations(): Promise<Automation[]> {
	const configs = listConfigs()
	const db = getDb()
	const results: Automation[] = []
	for (const config of configs) {
		const timing = await db.select().from(automations).where(eq(automations.id, config.id)).get()
		results.push(await mergeAutomation(config, timing))
	}
	return results
}

export async function getAutomation(id: string): Promise<Automation | null> {
	const config = readConfig(id)
	if (!config) return null
	const db = getDb()
	const timing = await db.select().from(automations).where(eq(automations.id, id)).get()
	return await mergeAutomation(config, timing)
}

export async function createAutomation(input: CreateAutomationInput): Promise<Automation> {
	const id = createConfig(input)
	const db = getDb()
	const now = Date.now()

	await db
		.insert(automations)
		.values({
			id,
			createdAt: now,
			updatedAt: now,
		})
		.run()

	// Schedule if active
	const config = readConfig(id)!
	if (config.status === "active") {
		await scheduleAndPersistNextRun(id, config.schedule.rrule, config.schedule.timezone)
	}

	return (await getAutomation(id))!
}

export async function updateAutomation(input: UpdateAutomationInput): Promise<Automation | null> {
	const before = readConfig(input.id)
	if (!before) return null

	updateConfig(input)
	const db = getDb()
	await db
		.update(automations)
		.set({ updatedAt: Date.now() })
		.where(eq(automations.id, input.id))
		.run()

	const after = readConfig(input.id)!

	// Re-schedule if schedule or status changed
	if (input.schedule || input.status) {
		removeTask(input.id)
		if (after.status === "active") {
			await scheduleAndPersistNextRun(input.id, after.schedule.rrule, after.schedule.timezone)
		} else {
			// Paused/archived: clear nextRunAt
			await db
				.update(automations)
				.set({ nextRunAt: null, updatedAt: Date.now() })
				.where(eq(automations.id, input.id))
				.run()
		}
	}

	return getAutomation(input.id)
}

export async function deleteAutomation(id: string): Promise<boolean> {
	removeTask(id)
	deleteConfig(id)
	const db = getDb()
	await db.delete(automations).where(eq(automations.id, id)).run()
	return true
}

// ============================================================
// Run operations
// ============================================================

export async function listRuns(automationId?: string, limit = 50): Promise<AutomationRun[]> {
	const db = getDb()

	if (automationId) {
		return (await db
			.select()
			.from(automationRuns)
			.where(eq(automationRuns.automationId, automationId))
			.limit(limit)
			.all()) as AutomationRun[]
	}

	return (await db.select().from(automationRuns).limit(limit).all()) as AutomationRun[]
}

export async function archiveRun(
	runId: string,
	reason: "auto" | "manual" = "manual",
): Promise<boolean> {
	const db = getDb()
	const result = await db
		.update(automationRuns)
		.set({
			status: "archived",
			archivedReason: reason,
			completedAt: Date.now(),
			updatedAt: Date.now(),
		})
		.where(eq(automationRuns.id, runId))
		.run()
	return result.rowsAffected > 0
}

export async function acceptRun(runId: string): Promise<boolean> {
	const db = getDb()
	const result = await db
		.update(automationRuns)
		.set({
			status: "accepted",
			completedAt: Date.now(),
			updatedAt: Date.now(),
		})
		.where(eq(automationRuns.id, runId))
		.run()
	return result.rowsAffected > 0
}

/** Mark a run as read without changing its status. */
export async function markRunRead(runId: string): Promise<boolean> {
	const db = getDb()
	const result = await db
		.update(automationRuns)
		.set({
			readAt: Date.now(),
			updatedAt: Date.now(),
		})
		.where(eq(automationRuns.id, runId))
		.run()
	return result.rowsAffected > 0
}

/**
 * Trigger an immediate automation run.
 *
 * Unlike scheduled runs, this works on paused automations too (it is a
 * manual trigger). The execution is fire-and-forget: the function returns
 * immediately after inserting the run record so the IPC handler does not
 * block the renderer.
 */
export async function runNow(id: string): Promise<boolean> {
	const config = readConfig(id)
	if (!config) return false

	// Fire-and-forget: start execution in the background
	executeAutomation(id, { isManualTrigger: true }).catch((err) => {
		log.error("runNow background execution failed", { automationId: id, error: err })
	})

	return true
}

export { previewSchedule }

// ============================================================
// Execution -- runs agent sessions via Devo SDK
// ============================================================

interface ExecuteOptions {
	/** When true, skip the active-status check (used by runNow). */
	isManualTrigger?: boolean
}

async function executeAutomation(id: string, opts: ExecuteOptions = {}): Promise<void> {
	const config = readConfig(id)
	if (!config) {
		log.warn("Automation config not found, skipping execution", { id })
		return
	}

	// Scheduled runs require active status; manual triggers skip the check
	if (!opts.isManualTrigger && config.status !== "active") {
		log.info("Automation not active, skipping execution", { id, status: config.status })
		return
	}

	const db = getDb()

	log.info("Waiting for semaphore", {
		automationId: id,
		isManual: !!opts.isManualTrigger,
		semaphoreActive: semaphore.active,
		semaphorePending: semaphore.pending,
	})
	const semaphoreWaitStart = Date.now()
	const release = await semaphore.acquire()
	const semaphoreWaitMs = Date.now() - semaphoreWaitStart
	log.info("Semaphore acquired", {
		automationId: id,
		waitMs: semaphoreWaitMs,
		semaphoreActive: semaphore.active,
	})

	try {
		// Run once per workspace, or once with empty workspace if none configured
		const targets = config.workspaces.length > 0 ? config.workspaces : [""]
		log.info("Starting automation execution", {
			automationId: id,
			name: config.name,
			workspaceCount: targets.length,
			retries: config.execution.retries,
			timeoutSec: config.execution.timeout,
		})
		for (const workspace of targets) {
			const runId = crypto.randomUUID()
			const now = Date.now()

			await db
				.insert(automationRuns)
				.values({
					id: runId,
					automationId: id,
					workspace,
					status: "running",
					attempt: 1,
					startedAt: now,
					timeoutAt: now + config.execution.timeout * 1000,
					createdAt: now,
					updatedAt: now,
				})
				.run()

			log.info("Automation run started", { automationId: id, runId, workspace })

			// Notify renderer that a new run appeared
			broadcastRunsUpdated()

			let runFailed = false

			try {
				const maxAttempts = Math.max(1, config.execution.retries + 1)
				let lastResult: Awaited<ReturnType<typeof executeRun>> | null = null

				for (let attempt = 1; attempt <= maxAttempts; attempt++) {
					if (attempt > 1) {
						// Update attempt number in DB
						await db
							.update(automationRuns)
							.set({ attempt, updatedAt: Date.now() })
							.where(eq(automationRuns.id, runId))
							.run()

						log.info("Retrying automation run", {
							automationId: id,
							runId,
							attempt,
							maxAttempts,
						})

						// Wait before retrying
						await new Promise((resolve) => setTimeout(resolve, config.execution.retryDelay * 1000))
					}

					lastResult = await executeRun(config, workspace, async (info) => {
						// Persist sessionId as soon as it is available so the
						// renderer can show the live session view immediately
						await db
							.update(automationRuns)
							.set({
								sessionId: info.sessionId,
								worktreePath: info.worktreePath,
								updatedAt: Date.now(),
							})
							.where(eq(automationRuns.id, runId))
							.run()

						broadcastRunsUpdated()
					})

					// If no error, break out of retry loop
					if (!lastResult.error) break

					// If this was the last attempt, don't retry
					if (attempt >= maxAttempts) break

					log.warn("Automation run attempt failed, will retry", {
						automationId: id,
						runId,
						attempt,
						error: lastResult.error,
					})
				}

				const result = lastResult!

				if (result.error) {
					// Run completed with error (all retries exhausted)
					runFailed = true
					await db
						.update(automationRuns)
						.set({
							status: "failed",
							errorMessage: result.error,
							resultTitle: result.title,
							resultSummary: result.summary || result.error,
							completedAt: Date.now(),
							updatedAt: Date.now(),
						})
						.where(eq(automationRuns.id, runId))
						.run()

					log.warn("Automation run failed", {
						automationId: id,
						runId,
						error: result.error,
					})
				} else if (!result.hasActionable) {
					// Nothing actionable -- auto-archive
					await db
						.update(automationRuns)
						.set({
							status: "archived",
							archivedReason: "auto",
							archivedAssistantMessage: result.summary,
							resultTitle: result.title,
							resultSummary: result.summary,
							resultHasActionable: false,
							resultBranch: result.branch,
							completedAt: Date.now(),
							updatedAt: Date.now(),
						})
						.where(eq(automationRuns.id, runId))
						.run()

					log.info("Automation run auto-archived (not actionable)", {
						automationId: id,
						runId,
					})
				} else {
					// Actionable results -- mark for human review
					await db
						.update(automationRuns)
						.set({
							status: "pending_review",
							resultTitle: result.title,
							resultSummary: result.summary,
							resultHasActionable: true,
							resultBranch: result.branch,
							completedAt: Date.now(),
							updatedAt: Date.now(),
						})
						.where(eq(automationRuns.id, runId))
						.run()

					log.info("Automation run completed, pending review", {
						automationId: id,
						runId,
					})
				}
			} catch (err) {
				// Unexpected error during execution
				runFailed = true
				await db
					.update(automationRuns)
					.set({
						status: "failed",
						errorMessage: err instanceof Error ? err.message : "Unknown error",
						completedAt: Date.now(),
						updatedAt: Date.now(),
					})
					.where(eq(automationRuns.id, runId))
					.run()

				log.error("Automation run threw unexpected error", {
					automationId: id,
					runId,
					error: err,
				})
			}

			// Update run count and consecutive failures
			if (runFailed) {
				await db
					.update(automations)
					.set({
						runCount: sql`${automations.runCount} + 1`,
						consecutiveFailures: sql`${automations.consecutiveFailures} + 1`,
						lastRunAt: Date.now(),
						updatedAt: Date.now(),
					})
					.where(eq(automations.id, id))
					.run()
			} else {
				await db
					.update(automations)
					.set({
						runCount: sql`${automations.runCount} + 1`,
						consecutiveFailures: 0,
						lastRunAt: Date.now(),
						updatedAt: Date.now(),
					})
					.where(eq(automations.id, id))
					.run()
			}

			// Notify renderer that the run completed
			broadcastRunsUpdated()
		}

		// Re-persist nextRunAt (compute fresh since scheduler reschedules asynchronously)
		const afterConfig = readConfig(id)
		if (afterConfig && afterConfig.status === "active") {
			const [nextDate] = await previewSchedule(
				afterConfig.schedule.rrule,
				afterConfig.schedule.timezone,
				1,
			)
			await db
				.update(automations)
				.set({
					nextRunAt: nextDate ? nextDate.getTime() : null,
					updatedAt: Date.now(),
				})
				.where(eq(automations.id, id))
				.run()
			log.info("Execution complete, next run persisted", {
				automationId: id,
				nextRunAt: nextDate?.toISOString() ?? "none",
			})
		} else {
			log.info("Execution complete, automation no longer active", {
				automationId: id,
				status: afterConfig?.status ?? "deleted",
			})
		}
	} finally {
		release()
		log.debug("Semaphore released", {
			automationId: id,
			semaphoreActive: semaphore.active,
			semaphorePending: semaphore.pending,
		})
	}
}

// ============================================================
// Helpers
// ============================================================

async function mergeAutomation(
	config: NonNullable<ReturnType<typeof readConfig>>,
	timing?: typeof automations.$inferSelect | undefined,
): Promise<Automation> {
	// Prefer in-memory scheduler value (always up-to-date) over DB value (may be stale)
	const schedulerNext = getNextRunTime(config.id)
	let nextRunAt = schedulerNext ? schedulerNext.getTime() : (timing?.nextRunAt ?? null)

	// If the automation is active but neither the scheduler nor the DB has a future
	// nextRunAt, compute it directly from the rrule. This covers the startup race
	// where initAutomations hasn't finished yet and the DB value is stale/null.
	if (config.status === "active" && (nextRunAt === null || nextRunAt <= Date.now())) {
		const preview = await previewSchedule(config.schedule.rrule, config.schedule.timezone, 1)
		if (preview.length > 0) {
			nextRunAt = preview[0].getTime()
		}
	}

	return {
		id: config.id,
		name: config.name,
		prompt: config.prompt,
		status: config.status,
		schedule: config.schedule,
		workspaces: config.workspaces,
		execution: config.execution,
		nextRunAt,
		lastRunAt: timing?.lastRunAt ?? null,
		runCount: timing?.runCount ?? 0,
		consecutiveFailures: timing?.consecutiveFailures ?? 0,
		createdAt: timing?.createdAt ?? Date.now(),
		updatedAt: timing?.updatedAt ?? Date.now(),
	}
}
