/** In-process automation scheduler. Explicit app quit always stops scheduling. */
import { scheduleOccurrences } from "../../shared/automation-schedule"
import { createLogger } from "../logger"

const log = createLogger("automation-scheduler")
// Bounded checkpoints avoid Node's ~24.8-day timeout overflow and recheck the
// wall clock after sleep or clock adjustments. They never execute early.
export const MAX_TIMER_DELAY_MS = 60_000

type Timer = ReturnType<typeof setTimeout>
interface ScheduledTask {
	timer: Timer | null
	rruleStr: string
	timezone: string
	startAt: Date
	paused: boolean
	generation: number
	callback: () => Promise<void>
	nextRunAt: Date | null
}

interface SchedulerDependencies {
	now: () => number
	setTimer: (callback: () => void, delay: number) => Timer
	clearTimer: (timer: Timer) => void
	occurrences: typeof scheduleOccurrences
}

/** Owns timer lifetimes; injected clock/timers make cancellation races testable. */
export class AutomationScheduler {
	private readonly tasks = new Map<string, ScheduledTask>()
	// Kept separately so replacing a schedule cannot overlap its in-flight run.
	private readonly running = new Set<string>()

	constructor(
		private readonly dependencies: SchedulerDependencies = {
			now: Date.now,
			setTimer: setTimeout,
			clearTimer: clearTimeout,
			occurrences: scheduleOccurrences,
		},
	) {}

	private isCurrent(id: string, task: ScheduledTask, generation: number): boolean {
		return this.tasks.get(id) === task && !task.paused && task.generation === generation
	}

	private async scheduleNext(id: string, task: ScheduledTask): Promise<Date | null> {
		const generation = ++task.generation
		if (task.timer !== null) this.dependencies.clearTimer(task.timer)
		task.timer = null
		task.nextRunAt = null
		if (task.paused || this.tasks.get(id) !== task) return null

		try {
			const [next] = await this.dependencies.occurrences(task.rruleStr, task.timezone, {
				after: new Date(this.dependencies.now()),
				count: 1,
				startAt: task.startAt,
			})
			// Removing, pausing, replacing or stopping while date math awaits must
			// not resurrect a timer, even if a pause was followed by a resume.
			if (!this.isCurrent(id, task, generation)) return null
			task.nextRunAt = next ?? null
			if (next) this.armTimer(id, task, generation, next)
			return next ?? null
		} catch (err) {
			log.error("Unable to schedule automation", { id, timezone: task.timezone }, err)
			return null
		}
	}

	private armTimer(id: string, task: ScheduledTask, generation: number, next: Date): void {
		const delay = Math.min(
			MAX_TIMER_DELAY_MS,
			Math.max(0, next.getTime() - this.dependencies.now()),
		)
		task.timer = this.dependencies.setTimer(() => {
			if (!this.isCurrent(id, task, generation)) return
			task.timer = null
			if (this.dependencies.now() < next.getTime()) {
				this.armTimer(id, task, generation, next)
				return
			}
			void this.fire(id, task, generation)
		}, delay)
		task.timer?.unref?.()
	}

	private async fire(id: string, task: ScheduledTask, generation: number): Promise<void> {
		task.nextRunAt = null
		if (this.running.has(id)) {
			// Skip an overlapping occurrence, never queue a backlog of side effects.
			await this.scheduleNext(id, task)
			return
		}
		this.running.add(id)
		try {
			await task.callback()
		} catch (err) {
			log.error("Automation callback threw", { id }, err)
		} finally {
			this.running.delete(id)
			if (this.isCurrent(id, task, generation)) await this.scheduleNext(id, task)
		}
	}

	async addTask(
		id: string,
		rruleStr: string,
		timezone: string,
		callback: () => Promise<void>,
	): Promise<Date | null> {
		this.removeTask(id)
		const task: ScheduledTask = {
			timer: null,
			rruleStr,
			timezone,
			callback,
			startAt: new Date(this.dependencies.now()),
			paused: false,
			generation: 0,
			nextRunAt: null,
		}
		this.tasks.set(id, task)
		return this.scheduleNext(id, task)
	}

	removeTask(id: string): void {
		const task = this.tasks.get(id)
		if (task?.timer !== null && task?.timer !== undefined) this.dependencies.clearTimer(task.timer)
		this.tasks.delete(id)
	}

	pauseTask(id: string): void {
		const task = this.tasks.get(id)
		if (!task) return
		task.paused = true
		task.generation++
		task.nextRunAt = null
		if (task.timer !== null) this.dependencies.clearTimer(task.timer)
		task.timer = null
	}

	resumeTask(id: string): void {
		const task = this.tasks.get(id)
		if (!task?.paused) return
		task.paused = false
		void this.scheduleNext(id, task)
	}

	isRunning(id: string): boolean {
		return this.running.has(id)
	}

	getNextRunTime(id: string): Date | null {
		return this.tasks.get(id)?.nextRunAt ?? null
	}

	stopAll(): void {
		for (const id of this.tasks.keys()) this.removeTask(id)
	}
}

const scheduler = new AutomationScheduler()
export const addTask = scheduler.addTask.bind(scheduler)
export const removeTask = scheduler.removeTask.bind(scheduler)
export const pauseTask = scheduler.pauseTask.bind(scheduler)
export const resumeTask = scheduler.resumeTask.bind(scheduler)
export const isRunning = scheduler.isRunning.bind(scheduler)
export const getNextRunTime = scheduler.getNextRunTime.bind(scheduler)
export const stopAll = scheduler.stopAll.bind(scheduler)

export async function previewSchedule(
	rruleStr: string,
	timezone: string,
	count = 5,
): Promise<Date[]> {
	try {
		return await scheduleOccurrences(rruleStr, timezone, { count })
	} catch (err) {
		log.warn("Invalid automation schedule", { rruleStr, timezone }, err)
		return []
	}
}
