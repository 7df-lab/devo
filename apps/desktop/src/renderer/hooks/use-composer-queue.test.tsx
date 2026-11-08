import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { sessionQueueFamily } from "../atoms/queue"
import type { QueueWireEntry } from "../lib/queue-helpers"

const dom = new Window({ url: "http://localhost" })
const original = Object.getOwnPropertyDescriptors(globalThis)
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const list = mock(async (_params: unknown) => ({ data: { entries: [] } }))
const update = mock(async (_params: unknown) => ({}))
const remove = mock(async (_params: unknown) => ({}))
const steer = mock(async (_params: unknown) => ({}))
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => ({ session: { queue: { list, update, remove }, steer } }),
}))
const { useComposerQueue } = await import("./use-composer-queue")
let queue: ReturnType<typeof useComposerQueue>
let root: Root
let store: ReturnType<typeof createStore>
function Harness({ sessionId }: { sessionId: string }) {
	queue = useComposerQueue(sessionId, "/repo")
	return null
}
const entries: QueueWireEntry[] = ["first", "second", "third"].map((text, index) => ({
	queueItemId: `queue-${index + 1}`,
	position: index + 1,
	preview: text,
	input: [{ type: "text", text }],
}))
async function mount(input = entries) {
	store = createStore()
	store.set(sessionQueueFamily("session-1"), input)
	store.set(sessionQueueFamily("session-2"), input)
	root = createRoot(dom.document.createElement("div") as unknown as HTMLElement)
	await navigate("session-1")
}
async function navigate(sessionId: string) {
	await act(async () => {
		root.render(<Provider store={store}><Harness sessionId={sessionId} /></Provider>)
	})
}
beforeEach(() => {
	for (const fn of [list, update, remove, steer]) fn.mockClear()
	update.mockImplementation(async () => ({}))
	remove.mockImplementation(async () => ({}))
	steer.mockImplementation(async () => ({}))
})
afterEach(async () => { await act(async () => { root?.unmount() }) })
afterAll(() => {
	mock.restore()
	for (const key of ["window", "document", "navigator", "IS_REACT_ACT_ENVIRONMENT"]) {
		if (original[key]) Object.defineProperty(globalThis, key, original[key])
		else Reflect.deleteProperty(globalThis, key)
	}
	void dom.happyDOM.close()
})

describe("composer queue actions", () => {
	test("reorders upward and downward using one-based Native positions", async () => {
		await mount()
		await act(async () => { await queue.reorderQueueItem("queue-1", "queue-2") })
		await act(async () => { await queue.reorderQueueItem("queue-1", "queue-3") })
		await act(async () => { await queue.reorderQueueItem("queue-3", "queue-1") })
		expect(update.mock.calls).toEqual([
			[{ sessionId: "session-1", queueItemId: "queue-1", position: 2 }],
			[{ sessionId: "session-1", queueItemId: "queue-1", position: 3 }],
			[{ sessionId: "session-1", queueItemId: "queue-3", position: 1 }],
		])
	})

	test("steers by queue identity rather than the lossy text preview", async () => {
		await mount([{ ...entries[0], input: [{ type: "image" }, { type: "skill" }] }])
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect([steer.mock.calls, remove.mock.calls]).toEqual([
			[[{ sessionId: "session-1", queueItemId: "queue-1" }]],
			[[{ sessionId: "session-1", queueItemId: "queue-1" }]],
		])
	})

	test("reorders the original message if an earlier entry drains during dragging", async () => {
		await mount()
		await act(async () => { store.set(sessionQueueFamily("session-1"), entries.slice(1)) })
		await act(async () => { await queue.reorderQueueItem("queue-2", "queue-3") })
		expect(update.mock.calls).toEqual([[{
			sessionId: "session-1", queueItemId: "queue-2", position: 2,
		}]])
	})

	test("does not reorder a replacement when the dragged message has already drained", async () => {
		await mount()
		await act(async () => { store.set(sessionQueueFamily("session-1"), entries.slice(1)) })
		await act(async () => { await queue.reorderQueueItem("queue-1", "queue-3") })
		expect(update.mock.calls).toEqual([])
	})

	test("keeps non-text inputs in the queue when text-only editing is requested", async () => {
		await mount([{ ...entries[0], input: [{ type: "skill" }] }])
		let result: string | null | undefined
		await act(async () => { result = await queue.editQueueItem(queue.queueItems[0]) })
		expect([queue.queueItems[0].canEdit, result, remove.mock.calls]).toEqual([false, null, []])
	})

	test("does not inject twice on repeated clicks or removal retry", async () => {
		await mount()
		let finishSteer!: () => void
		steer.mockImplementationOnce(() => new Promise((resolve) => {
			finishSteer = () => resolve({})
		}))
		remove.mockImplementationOnce(async () => { throw new Error("Remove unavailable") })
		const item = queue.queueItems[0]
		await act(async () => {
			const first = queue.steerQueueItem(item)
			await queue.steerQueueItem(item)
			finishSteer()
			await first
		})
		expect([queue.queueItems[0].error, queue.queueItems[0].steered, queue.queueItems[0].canEdit]).toEqual([
			"Remove unavailable", true, false,
		])
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect([steer.mock.calls.length, remove.mock.calls.length, queue.queueItems[0].error]).toEqual([1, 2, undefined])
	})

	test("retains the original queued message when steering fails", async () => {
		await mount()
		steer.mockImplementationOnce(async () => { throw new Error("Turn finished") })
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect([queue.queueItems[0].error, queue.queueItems[0].status, remove.mock.calls]).toEqual([
			"Turn finished", "queued", [],
		])
	})

	test("retains removal-only retry state when navigating away and back", async () => {
		await mount()
		remove.mockImplementationOnce(async () => { throw new Error("Remove unavailable") })
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		await navigate("session-2")
		expect([queue.queueItems[0].steered, queue.queueItems[0].error]).toEqual([false, undefined])
		await navigate("session-1")
		expect([queue.queueItems[0].steered, queue.queueItems[0].error]).toEqual([true, "Remove unavailable"])
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect([steer.mock.calls.length, remove.mock.calls.length]).toEqual([1, 2])
	})

	test("finishes pending actions in their original session after navigation", async () => {
		await mount()
		let finishSteer!: () => void
		steer.mockImplementationOnce(() => new Promise((resolve) => {
			finishSteer = () => resolve({})
		}))
		remove.mockImplementationOnce(async () => { throw new Error("Remove unavailable") })
		let pending!: Promise<void>
		await act(async () => { pending = queue.steerQueueItem(queue.queueItems[0]) })
		await navigate("session-2")
		expect(queue.queueItems[0].status).toBe("queued")
		await act(async () => { finishSteer(); await pending })
		expect([queue.queueItems[0].steered, queue.queueItems[0].error]).toEqual([false, undefined])
		await navigate("session-1")
		expect([queue.queueItems[0].steered, queue.queueItems[0].error]).toEqual([true, "Remove unavailable"])
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect([steer.mock.calls.length, remove.mock.calls.length]).toEqual([1, 2])
	})

	test("reports removal failure without rejecting a fire-and-forget click or restoring a draft", async () => {
		await mount()
		remove.mockImplementation(async () => { throw new Error("Offline") })
		let removed: boolean | undefined
		let text: string | null | undefined
		await act(async () => { removed = await queue.removeQueueItem(queue.queueItems[0]) })
		await act(async () => { text = await queue.editQueueItem(queue.queueItems[0]) })
		expect([removed, text, queue.queueItems[0].error]).toEqual([false, null, "Offline"])
	})

	test("reports failed reordering and refreshes authoritative order", async () => {
		await mount()
		update.mockImplementationOnce(async () => { throw new Error("Reorder unavailable") })
		await act(async () => { await queue.reorderQueueItem("queue-1", "queue-3") })
		expect([queue.queueItems[0].error, queue.queueItems[0].status, list.mock.calls.length]).toEqual([
			"Reorder unavailable", "queued", 2,
		])
	})
})
