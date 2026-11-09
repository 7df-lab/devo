import { describe, expect, test } from "bun:test"
import { AutomationScheduler, MAX_TIMER_DELAY_MS } from "./scheduler"

function deferred<T>() {
	let resolve!: (value: T) => void
	const promise = new Promise<T>((done) => {
		resolve = done
	})
	return { promise, resolve }
}

async function settle() {
	for (let i = 0; i < 12; i++) await Promise.resolve()
}

function harness(nextRunAt: number) {
	let now = 0
	const timers = new Map<ReturnType<typeof setTimeout>, { callback: () => void; delay: number }>()
	let occurrence: () => Promise<Date[]> = async () => [new Date(nextRunAt)]
	const scheduler = new AutomationScheduler({
		now: () => now,
		occurrences: () => occurrence(),
		setTimer: (callback, delay) => {
			const timer = { unref() {} } as ReturnType<typeof setTimeout>
			timers.set(timer, { callback, delay })
			return timer
		},
		clearTimer: (timer) => {
			timers.delete(timer)
		},
	})
	return {
		scheduler,
		timers,
		setNow(value: number) {
			now = value
		},
		setOccurrence(value: () => Promise<Date[]>) {
			occurrence = value
		},
		async fire() {
			const scheduled = timers.entries().next().value
			if (!scheduled) throw new Error("No timer is armed")
			const [timer, entry] = scheduled
			timers.delete(timer)
			entry.callback()
			await settle()
		},
	}
}

describe("automation timer ownership", () => {
	test("chunks a delay beyond Node's timer limit and never fires early", async () => {
		const due = 40 * 24 * 60 * 60 * 1000
		const h = harness(due)
		let runs = 0
		await h.scheduler.addTask("a", "rule", "UTC", async () => {
			runs++
		})
		expect([...h.timers.values()].map((timer) => timer.delay)).toEqual([MAX_TIMER_DELAY_MS])
		h.setNow(MAX_TIMER_DELAY_MS)
		await h.fire()
		expect(runs).toBe(0)
		expect(h.scheduler.getNextRunTime("a")).toEqual(new Date(due))
		h.setOccurrence(async () => [])
		h.setNow(due)
		await h.fire()
		expect(runs).toBe(1)
		expect(h.timers.size).toBe(0)
	})

	test("clock rollback re-arms without early execution", async () => {
		const h = harness(1000)
		let runs = 0
		await h.scheduler.addTask("a", "rule", "UTC", async () => {
			runs++
		})
		h.setNow(-5000)
		await h.fire()
		expect(runs).toBe(0)
		expect([...h.timers.values()].map((timer) => timer.delay)).toEqual([6000])
	})

	test("sleep past several occurrences causes one run, then computes a future occurrence", async () => {
		const h = harness(1000)
		let runs = 0
		await h.scheduler.addTask("a", "rule", "UTC", async () => {
			runs++
		})
		h.setNow(10_000)
		h.setOccurrence(async () => [new Date(11_000)])
		await h.fire()
		expect(runs).toBe(1)
		expect(h.scheduler.getNextRunTime("a")).toEqual(new Date(11_000))
	})

	for (const action of ["remove", "pause", "stop"] as const) {
		test(`${action} during asynchronous date calculation cannot resurrect a timer`, async () => {
			const h = harness(1000)
			const pending = deferred<Date[]>()
			h.setOccurrence(() => pending.promise)
			const added = h.scheduler.addTask("a", "rule", "UTC", async () => {})
			if (action === "remove") h.scheduler.removeTask("a")
			if (action === "pause") h.scheduler.pauseTask("a")
			if (action === "stop") h.scheduler.stopAll()
			pending.resolve([new Date(1000)])
			expect(await added).toBeNull()
			expect(h.timers.size).toBe(0)
			expect(h.scheduler.getNextRunTime("a")).toBeNull()
		})
	}

	test("pause/resume invalidates the first pending computation", async () => {
		const h = harness(1000)
		const pending = deferred<Date[]>()
		h.setOccurrence(() => pending.promise)
		const added = h.scheduler.addTask("a", "rule", "UTC", async () => {})
		h.scheduler.pauseTask("a")
		h.setOccurrence(async () => [new Date(2000)])
		h.scheduler.resumeTask("a")
		await settle()
		pending.resolve([new Date(1000)])
		await added
		expect(h.timers.size).toBe(1)
		expect(h.scheduler.getNextRunTime("a")).toEqual(new Date(2000))
	})

	test("replacing an in-flight task prevents overlapping callbacks", async () => {
		const h = harness(1000)
		const running = deferred<void>()
		let replacementRuns = 0
		await h.scheduler.addTask("a", "rule", "UTC", () => running.promise)
		h.setNow(1000)
		await h.fire()
		expect(h.scheduler.isRunning("a")).toBe(true)
		h.setOccurrence(async () => [new Date(2000)])
		await h.scheduler.addTask("a", "new rule", "UTC", async () => {
			replacementRuns++
		})
		h.setNow(2000)
		h.setOccurrence(async () => [new Date(3000)])
		await h.fire()
		expect(replacementRuns).toBe(0)
		running.resolve()
		await settle()
		expect(h.timers.size).toBe(1)
		h.setNow(3000)
		h.setOccurrence(async () => [])
		await h.fire()
		expect(replacementRuns).toBe(1)
	})

	test("callback failure keeps the recurring timer chain alive", async () => {
		const h = harness(1000)
		await h.scheduler.addTask("a", "rule", "UTC", async () => {
			throw new Error("execution failed")
		})
		h.setNow(1000)
		h.setOccurrence(async () => [new Date(2000)])
		await h.fire()
		expect(h.scheduler.isRunning("a")).toBe(false)
		expect(h.scheduler.getNextRunTime("a")).toEqual(new Date(2000))
	})

	test("stop during an in-flight callback prevents rescheduling", async () => {
		const h = harness(1000)
		const running = deferred<void>()
		await h.scheduler.addTask("a", "rule", "UTC", () => running.promise)
		h.setNow(1000)
		await h.fire()
		h.scheduler.stopAll()
		running.resolve()
		await settle()
		expect(h.timers.size).toBe(0)
		expect(h.scheduler.isRunning("a")).toBe(false)
	})
})
