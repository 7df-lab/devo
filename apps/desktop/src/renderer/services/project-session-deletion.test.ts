import { describe, expect, test } from "bun:test"
import { deleteProjectSessionBatch } from "./project-session-deletion"

describe("folder session deletion", () => {
	test("starts eight deletions together and refills the worker pool", async () => {
		const sessionIds = Array.from({ length: 19 }, (_, index) => `session-${index}`)
		const started: string[] = []
		const deleted: string[] = []
		const releases: Array<() => void> = []
		let active = 0
		let maximumActive = 0
		const pending = deleteProjectSessionBatch({
			sessionIds,
			deleteSession: async (id) => {
				started.push(id)
				active += 1
				maximumActive = Math.max(maximumActive, active)
				await new Promise<void>((resolve) => releases.push(resolve))
				active -= 1
			},
			onDeleted: (id) => deleted.push(id),
		})
		expect(started).toEqual(sessionIds.slice(0, 8))
		while (deleted.length < sessionIds.length) {
			for (const release of releases.splice(0)) release()
			await new Promise<void>((resolve) => queueMicrotask(resolve))
		}
		await pending
		expect({ started, deleted, maximumActive, active }).toEqual({
			started: sessionIds, deleted: sessionIds, maximumActive: 8, active: 0,
		})
	})

	test("settles every deletion and retains failures for retry", async () => {
		const deleted: string[] = []
		const attempted: string[] = []
		const failure = new Error("permission denied")
		const result = await deleteProjectSessionBatch({
			sessionIds: ["failed", "second", "third"],
			deleteSession: async (id) => {
				attempted.push(id)
				if (id === "failed") throw failure
			},
			onDeleted: (id) => deleted.push(id),
		}).then(() => null, (error: AggregateError) => error)
		expect({ attempted, deleted, errors: result?.errors, message: result?.message }).toEqual({
			attempted: ["failed", "second", "third"], deleted: ["second", "third"],
			errors: [failure], message: "1 session deletion(s) failed: permission denied",
		})
	})

	test("accepts an empty folder", async () => {
		const calls: string[] = []
		await deleteProjectSessionBatch({
			sessionIds: [], deleteSession: async (id) => { calls.push(id) }, onDeleted: (id) => calls.push(id),
		})
		expect(calls).toEqual([])
	})
})
