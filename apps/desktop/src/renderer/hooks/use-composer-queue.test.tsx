import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { sessionActiveTurnFamily, sessionQueueFamily } from "../atoms/queue"
import { ComposerStatusStack } from "../components/chat/composer-status-stack"
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
const accepted = { data: { outcome: "accepted" } }
const steer = mock(async (_params: unknown) => accepted)
const retry = mock(async (_params: unknown) => ({}))
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => ({ session: { queue: { list, update, remove, steer, retry } } }),
}))
const { useComposerQueue } = await import("./use-composer-queue")
let queue: ReturnType<typeof useComposerQueue>
let root: Root
let container: HTMLElement
let store: ReturnType<typeof createStore>
function Harness({ sessionId }: { sessionId: string }) {
	queue = useComposerQueue(sessionId, "/repo")
	return <ComposerStatusStack queueItems={queue.queueItems} onRetryQueueItem={queue.retryQueueItem} onRemoveQueueItem={queue.removeQueueItem} onEditQueueItem={queue.editQueueItem} onSteerQueueItem={queue.steerQueueItem} />
}
const entries: QueueWireEntry[] = ["first", "second", "third"].map((text, index) => ({
	queueItemId: `queue-${index + 1}`,
	position: index + 1,
	preview: text,
	input: [{ type: "text", text }],
}))
async function mount(input = entries) {
	store = createStore()
	store.set(sessionActiveTurnFamily("session-1"), "turn-1")
	store.set(sessionQueueFamily("session-1"), input)
	store.set(sessionQueueFamily("session-2"), input)
	container = dom.document.createElement("div") as unknown as HTMLElement
	root = createRoot(container)
	await navigate("session-1")
}
async function navigate(sessionId: string) {
	await act(async () => {
		root.render(<Provider store={store}><Harness sessionId={sessionId} /></Provider>)
	})
}
beforeEach(() => {
	for (const fn of [list, update, remove, steer, retry]) fn.mockClear()
	update.mockImplementation(async () => ({}))
	remove.mockImplementation(async () => ({}))
	steer.mockImplementation(async () => accepted)
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
			[[{ sessionId: "session-1", queueItemId: "queue-1", expectedTurnId: "turn-1" }]],
			[],
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

	test("locks repeated clicks and retries the same identity after an uncertain response", async () => {
		await mount()
		let fail!: () => void
		steer.mockImplementationOnce(() => new Promise((_, reject) => { fail = () => reject(new Error("Response lost")) }))
		const item = queue.queueItems[0]
		await act(async () => {
			const pending = queue.steerQueueItem(item)
			await queue.steerQueueItem(item)
			fail()
			await pending
		})
		expect(steer.mock.calls.length).toBe(1)
		expect(queue.queueItems[0].error).toBe("Response lost")
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect(steer.mock.calls[1]).toEqual(steer.mock.calls[0])
		expect(remove.mock.calls).toEqual([])
	})

	test("scopes late failures to the original session across navigation", async () => {
		await mount()
		let fail!: () => void
		steer.mockImplementationOnce(() => new Promise((_, reject) => { fail = () => reject(new Error("Offline")) }))
		let pending!: Promise<void>
		await act(async () => { pending = queue.steerQueueItem(queue.queueItems[0]) })
		await navigate("session-2")
		expect(queue.queueItems[0].status).toBe("queued")
		await act(async () => { fail(); await pending })
		expect(queue.queueItems[0].error).toBeUndefined()
		await navigate("session-1")
		expect(queue.queueItems[0].error).toBe("Offline")
	})

	test("keeps stale-turn messages queued without a deletion", async () => {
		await mount()
		steer.mockImplementationOnce(async () => ({ data: { outcome: "keptQueued" } }))
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect(queue.queueItems[0].error).toContain("still queued")
		expect(remove.mock.calls).toEqual([])
	})

	test("held messages require explicit recovery and cannot steer, edit, or reorder", async () => {
		await mount([{ ...entries[0], recoveryRequired: true }, entries[1], entries[2]])
		const item = queue.queueItems[0]
		await act(async () => {
			await queue.steerQueueItem(item)
			expect(await queue.editQueueItem(item)).toBeNull()
			await queue.reorderQueueItem(item.id, "queue-2")
			await queue.reorderQueueItem("queue-2", item.id)
			await queue.reorderQueueItem("queue-2", "queue-3")
		})
		expect([steer.mock.calls, remove.mock.calls, update.mock.calls, retry.mock.calls]).toEqual([[], [], [], []])
		await act(async () => { await queue.retryQueueItem(item) })
		expect(retry.mock.calls).toEqual([[{ sessionId: "session-1", queueItemId: "queue-1" }]])
		await act(async () => { await queue.removeQueueItem(item) })
		expect(remove.mock.calls.length).toBe(1)
	})

	test("rendered Retry is explicit and locked while recovery is pending", async () => {
		await mount([{ ...entries[0], recoveryRequired: true }])
		expect(container.textContent).toContain("Prior effects may have occurred")
		expect(retry.mock.calls).toEqual([])
		let finish!: () => void
		retry.mockImplementationOnce(() => new Promise((resolve) => { finish = () => resolve({}) }))
		const button = [...container.querySelectorAll("button")].find((item) => item.textContent === "Retry")
		if (!button) throw new Error("Recovery Retry button is missing")
		await act(async () => { button.click(); button.click() })
		expect(retry.mock.calls.length).toBe(1)
		expect(button.disabled).toBe(true)
		await act(async () => { finish() })
		expect(button.disabled).toBe(false)
	})

	test("remount retries the same queue identity without removal", async () => {
		await mount()
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]); root.unmount() })
		await mount()
		await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
		expect(steer.mock.calls[1]).toEqual(steer.mock.calls[0])
		expect(remove.mock.calls).toEqual([])
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


test("host queue entries retain origin and cannot be edited or steered as human input", async () => {
 const origin = { eventId: "host-event", source: "agent" as const, sender: "child", delivery: "followUp" as const }
 await mount([{ ...entries[0]!, origin }])
 expect(queue.queueItems[0]!.origin).toEqual(origin)
 await act(async () => {
  await queue.steerQueueItem(queue.queueItems[0]!)
  expect(await queue.editQueueItem(queue.queueItems[0]!)).toBeNull()
 })
 expect(steer).not.toHaveBeenCalled()
 expect(remove).not.toHaveBeenCalled()
})
