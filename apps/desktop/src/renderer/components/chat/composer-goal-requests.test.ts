import { describe, expect, test } from "bun:test"
import { ComposerGoalRequests } from "./composer-goal-requests"

describe("composer goal request ordering", () => {
	test("a delayed active status cannot resurrect a cleared goal", async () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		let displayed: string | null = "active"
		let resolve!: (value: string) => void
		const response = new Promise<string>((done) => { resolve = done })
		const read = requests.read()!
		const pending = response.then((goal) => {
			if (read.isCurrent()) displayed = goal
		})
		const clear = requests.mutate()!
		expect(requests.read()).toBeNull()
		if (clear.isCurrent()) displayed = null
		clear.finish()
		resolve("active")
		await pending
		expect(displayed).toBeNull()
		expect(requests.read()?.isCurrent()).toBe(true)
	})

	test("slow status survives multiple polling ticks without starvation", () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		const slow = requests.read()!
		expect([requests.read(), requests.read(), requests.read()]).toEqual([null, null, null])
		expect(slow.isCurrent()).toBe(true)
		slow.finish()
		expect(requests.read()?.isCurrent()).toBe(true)
	})

	test("session cleanup invalidates old reads, actions and delayed callbacks", () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		const read = requests.read()!
		const action = requests.mutate()!
		requests.dispose()
		expect([read.isCurrent(), action.isCurrent(), requests.read(), requests.mutate()])
			.toEqual([false, false, null, null])
	})

	test("StrictMode reactivation rejects old replies and old finalizers", () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		const old = requests.mutate()!
		requests.dispose()
		requests.activate()
		const current = requests.mutate()!
		old.finish()
		expect([old.isCurrent(), current.isCurrent(), requests.read()]).toEqual([false, true, null])
		current.finish()
		expect(requests.read()?.isCurrent()).toBe(true)
	})

	test("an obsolete read finalizer cannot unlock the next read", () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		const old = requests.read()!
		requests.mutate()!.finish()
		const current = requests.read()!
		old.finish()
		expect([old.isCurrent(), current.isCurrent(), requests.read()]).toEqual([false, true, null])
		current.finish()
		expect(requests.read()?.isCurrent()).toBe(true)
	})

	test("repeated actions wait until the first settles", () => {
		const requests = new ComposerGoalRequests()
		requests.activate()
		const pause = requests.mutate()!
		expect(requests.mutate()).toBeNull()
		pause.finish()
		expect(requests.mutate()?.isCurrent()).toBe(true)
	})
})
