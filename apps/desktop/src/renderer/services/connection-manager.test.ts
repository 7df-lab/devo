import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import type { DevoClient } from "@devo-ai/sdk/v2/client"
import type { Event, Session } from "../lib/types"
import { sessionFamily, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"

class FakeEventStream {
	private readonly queue: Array<{ payload: Event }> = []
	private readonly waiters: Array<(result: IteratorResult<{ payload: Event }>) => void> = []
	private closed = false

	push(payload: Event): void {
		const item = { payload }
		const waiter = this.waiters.shift()
		if (waiter) {
			waiter({ value: item, done: false })
			return
		}
		this.queue.push(item)
	}

	close(): void {
		this.closed = true
		for (const waiter of this.waiters.splice(0)) {
			waiter({ value: undefined, done: true })
		}
	}

	async *[Symbol.asyncIterator](): AsyncIterableIterator<{ payload: Event }> {
		while (true) {
			const queued = this.queue.shift()
			if (queued) {
				yield queued
				continue
			}
			if (this.closed) return
			const next = await new Promise<IteratorResult<{ payload: Event }>>((resolve) => {
				this.waiters.push(resolve)
			})
			if (next.done) return
			yield next.value
		}
	}
}

const streams = new Map<string, FakeEventStream>()
let activeManager: typeof import("./connection-manager") | null = null

function streamFor(directory: string): FakeEventStream {
	let stream = streams.get(directory)
	if (!stream) {
		stream = new FakeEventStream()
		streams.set(directory, stream)
	}
	return stream
}

mock.module("./devo", () => ({
	connectToServer: (_url: string, options?: { directory?: string }) =>
		({ directory: options?.directory ?? "__base__" }) as unknown as DevoClient,
	disposeAllInstances: () => {},
	getSession: async () => null,
	getSessionStatuses: async () => ({}),
	listProjects: async () => [],
	listSessions: async () => [],
	subscribeToGlobalEvents: async (client: DevoClient) =>
		streamFor(((client as unknown as { directory?: string }).directory) ?? "__base__"),
}))

describe("connection manager project status bridge", () => {
	beforeEach(() => {
		streams.clear()
		;(globalThis as unknown as { window: Record<string, unknown> }).window = {}
		;(globalThis as unknown as { requestAnimationFrame: (callback: FrameRequestCallback) => number }).requestAnimationFrame =
			(callback) => setTimeout(() => callback(performance.now()), 0) as unknown as number
		;(globalThis as unknown as { cancelAnimationFrame: (id: number) => void }).cancelAnimationFrame = (id) =>
			clearTimeout(id)
	})

	afterEach(async () => {
		activeManager?.disconnect()
		activeManager = null
		for (const stream of streams.values()) {
			stream.close()
		}
		delete (globalThis as unknown as { window?: unknown }).window
		delete (globalThis as unknown as { requestAnimationFrame?: unknown }).requestAnimationFrame
		delete (globalThis as unknown as { cancelAnimationFrame?: unknown }).cancelAnimationFrame
	})

	test("forwards project-scoped session status events into renderer state", async () => {
		const directory = "/repo/project-status-bridge"
		const session: Session = {
			id: "status-bridge-session",
			directory,
			title: "Status bridge",
			time: { created: 1, updated: 1 },
		}
		appStore.set(upsertSessionAtom, { session, directory })

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		expect(manager.getProjectClient(directory)).not.toBeNull()

		streamFor(directory).push({
			type: "session.status",
			properties: {
				sessionID: session.id,
				status: { type: "busy" },
			},
		})
		await new Promise((resolve) => setTimeout(resolve, 5))

		expect(appStore.get(sessionFamily(session.id))?.status).toEqual({ type: "busy" })
	})
})
