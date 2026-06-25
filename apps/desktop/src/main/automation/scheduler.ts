/**
 * Automation scheduler service.
 *
 * Uses the rrule library for date math and setTimeout for precise
 * per-automation timers. No polling -- each automation gets a single
 * timer set to fire at its exact next run time.
 */

import { createLogger } from "../logger"

const log = createLogger("automation-scheduler")

interface ScheduledTask {
	timer: ReturnType<typeof setTimeout> | null
	rruleStr: string
	timezone: string
	paused: boolean
	running: boolean
	callback: () => Promise<void>
	/** The computed next run time, updated after each scheduling cycle. */
	nextRunAt: Date | null
}

const tasks = new Map<string, ScheduledTask>()

// ============================================================
// Internal helpers
// ============================================================

/** Human-readable delay string for log messages. */
function formatDelay(ms: number): string {
	if (ms < 1000) return `${ms}ms`
	const seconds = Math.round(ms / 1000)
	if (seconds < 120) return `${seconds}s`
	const minutes = Math.round(seconds / 60)
	if (minutes < 120) return `${minutes}m`
	const hours = Math.round(minutes / 60)
	return `${hours}h`
}

async function computeNextRun(
	id: string,
	rruleStr: string,
	_timezone: string,
): Promise<Date | null> {
	const rruleModule = await import("rrule")
	// Handle CJS/ESM interop: named export in CJS, nested under default in ESM
	const RRule = rruleModule.RRule ?? rruleModule.default?.RRule
	try {
		const rule = RRule.fromString(rruleStr)
		const now = new Date()
		const next = rule.after(now, false)
		if (next) {
			const delayMs = next.getTime() - now.getTime()
			log.debug("Computed next run", {
				id,
				rruleStr,
				now: now.toISOString(),
				next: next.toISOString(),
				delayMs,
			})
		} else {
			log.warn("RRULE produced no future occurrence", { id, rruleStr, now: now.toISOString() })
		}
		return next
	} catch (err) {
		log.error("Failed to parse RRULE", { id, rruleStr }, err)
		return null
	}
}

async function scheduleNext(id: string, task: ScheduledTask): Promise<Date | null> {
	if (task.timer) {
		clearTimeout(task.timer)
		task.timer = null
		log.debug("Cleared previous timer", { id })
	}

	if (task.paused) {
		log.debug("Task is paused, skipping schedule", { id })
		task.nextRunAt = null
		return null
	}

	const next = await computeNextRun(id, task.rruleStr, task.timezone)
	if (!next) {
		log.warn("No next occurrence for automation, timer chain stopped", {
			id,
			rruleStr: task.rruleStr,
		})
		task.nextRunAt = null
		return null
	}

	task.nextRunAt = next
	const delay = Math.max(0, next.getTime() - Date.now())
	log.info("Timer set", {
		id,
		nextRun: next.toISOString(),
		delayMs: delay,
		delayHuman: formatDelay(delay),
	})

	task.timer = setTimeout(async () => {
		// Verify this task is still the active one (guards against stale closures
		// after addTask replaces the task while a callback is in-flight)
		if (tasks.get(id) !== task) {
			log.warn("Timer fired for stale task, ignoring", { id })
			return
		}
		if (task.paused) {
			log.warn("Timer fired but task is paused, ignoring", { id })
			return
		}
		if (task.running) {
			log.warn("Timer fired but task is already running, ignoring", { id })
			return
		}

		log.info("Timer fired, starting automation callback", { id, scheduledFor: next.toISOString() })
		task.running = true
		const startTime = Date.now()
		try {
			await task.callback()
			log.info("Automation callback completed", {
				id,
				durationMs: Date.now() - startTime,
			})
		} catch (err) {
			log.error("Automation callback threw", { id, durationMs: Date.now() - startTime }, err)
		} finally {
			task.running = false
			// Schedule the next run. Await and catch so a rejection doesn't
			// silently kill the recurring timer chain.
			if (tasks.get(id) === task && !task.paused) {
				log.debug("Rescheduling after run", { id })
				try {
					await scheduleNext(id, task)
				} catch (err) {
					log.error("Failed to reschedule automation after run -- timer chain broken", { id }, err)
				}
			} else {
				log.warn("Skipping reschedule", {
					id,
					reason: tasks.get(id) !== task ? "task replaced" : "task paused",
				})
			}
		}
	}, delay)

	// Don't prevent app exit
	if (task.timer && typeof task.timer === "object" && "unref" in task.timer) {
		task.timer.unref()
	}

	return next
}

// ============================================================
// Public API
// ============================================================

export async function addTask(
	id: string,
	rruleStr: string,
	timezone: string,
	callback: () => Promise<void>,
): Promise<Date | null> {
	log.info("Adding task", { id, rruleStr, timezone, hadExisting: tasks.has(id) })
	removeTask(id)
	const task: ScheduledTask = {
		timer: null,
		rruleStr,
		timezone,
		paused: false,
		running: false,
		callback,
		nextRunAt: null,
	}
	tasks.set(id, task)
	const next = await scheduleNext(id, task)
	log.info("Task added", {
		id,
		nextRun: next?.toISOString() ?? "none",
		totalTasks: tasks.size,
	})
	return next
}

export function removeTask(id: string): void {
	const task = tasks.get(id)
	if (task) {
		if (task.timer) clearTimeout(task.timer)
		tasks.delete(id)
		log.info("Task removed", { id, wasRunning: task.running, totalTasks: tasks.size })
	}
}

export function pauseTask(id: string): void {
	const task = tasks.get(id)
	if (task) {
		task.paused = true
		if (task.timer) {
			clearTimeout(task.timer)
			task.timer = null
		}
		log.info("Task paused", { id })
	}
}

export function resumeTask(id: string): void {
	const task = tasks.get(id)
	if (task?.paused) {
		task.paused = false
		log.info("Task resumed, rescheduling", { id })
		scheduleNext(id, task)
	}
}

export function isRunning(id: string): boolean {
	return tasks.get(id)?.running ?? false
}

/**
 * Returns the computed next run time for a scheduled task, or null if
 * the task doesn't exist, is paused, or has no future occurrences.
 */
export function getNextRunTime(id: string): Date | null {
	return tasks.get(id)?.nextRunAt ?? null
}

export function stopAll(): void {
	log.info("Stopping all scheduled automations", { count: tasks.size })
	for (const [id, task] of tasks) {
		if (task.timer) clearTimeout(task.timer)
		log.debug("Stopped task", { id, wasRunning: task.running })
	}
	tasks.clear()
	log.info("All scheduled automations stopped")
}

/**
 * Compute the next N occurrences for an RRULE (for UI preview).
 */
export async function previewSchedule(
	rruleStr: string,
	_timezone: string,
	count: number = 5,
): Promise<Date[]> {
	const rruleModule = await import("rrule")
	// Handle CJS/ESM interop: named export in CJS, nested under default in ESM
	const RRule = rruleModule.RRule ?? rruleModule.default?.RRule
	try {
		const rule = RRule.fromString(rruleStr)
		const dates: Date[] = []
		let current = new Date()
		for (let i = 0; i < count; i++) {
			const next = rule.after(current, false)
			if (!next) break
			dates.push(next)
			current = next
		}
		return dates
	} catch {
		return []
	}
}
