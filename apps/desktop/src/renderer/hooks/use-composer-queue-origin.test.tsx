import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { Provider } from "jotai"
import type { Root } from "react-dom/client"
import { createDevoClient, type DevoNativeTransportEvent, type HostMessageOrigin } from "@devo-ai/sdk/v2/client"
import { appStore } from "../atoms/store"
import { processEvent } from "../atoms/actions/event-processor"
import { ComposerStatusStack } from "../components/chat/composer-status-stack"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator, Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
const { createRoot } = await import("react-dom/client")
let client: ReturnType<typeof createDevoClient>
// Only connection lookup is injected. Queue parsing, Native validation, SDK events,
// event processing, atoms, hook actions, and rendered controls are production code.
mock.module("../services/connection-manager", () => ({ getProjectClient: () => client }))
const { useComposerQueue } = await import("./use-composer-queue")
let queue: ReturnType<typeof useComposerQueue>
let root: Root | undefined
const at = "2026-11-09T00:00:00Z"
const session = {
	id: "origin-session", version: 1, cwd: "/repo", title: "Origin test", parent: null,
	createdAt: at, lastActivityAt: at, status: "active", flags: [], archived: false, ephemeral: false,
	model: { provider: "test", model: "test" }, settings: { permissionProfile: "default" }, preview: "", queuedCount: 1,
	usage: { total: { inputTokens: 0, outputTokens: 0, cacheCreationInputTokens: 0, cacheReadInputTokens: 0, reasoningTokens: 0, totalTokens: 0, callCount: 0, meteredCallCount: 0, failedCallCount: 0, cancelledCallCount: 0 }, byPurpose: [], updatedAt: at },
}
function wireEntry(origin?: HostMessageOrigin) {
	return { queueItemId: "queue-1", position: 0, enqueuedAt: at, preview: "Result ready", input: [{ type: "text", text: "Result ready" }], ...(origin ? { origin } : {}) }
}
class Transport {
	listener?: (event: DevoNativeTransportEvent) => void
	requests: Array<{ method: string; params: any }> = []
	entry = wireEntry()
	snapshot = false
	async request(method: string, params?: any): Promise<unknown> {
		this.requests.push({ method, params })
		switch (method) {
			case "initialize": return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			case "session/resume": return { session }
			case "subscription/create": return { subscriptionId: "sub-1", cursors: [], replay: [], pendingControlRequests: [], recoverySnapshots: [], snapshots: this.snapshot ? [{ streamId: "session:origin-session", barrierSeq: 0, data: { kind: "session", session, queue: [this.entry] } }] : [] }
			case "subscription/ack": return { serverTimeMs: 0 }
			case "session/queue/list": return { entries: [this.entry] }
			case "session/queue/update": return { entry: { ...this.entry, position: params.position ?? 0 } }
			case "session/queue/remove": return {}
			case "session/queue/push": return { outcome: "queued", entry: this.entry }
			case "session/queue/steer": return { outcome: "accepted", queueItemId: params.queueItemId, turnId: params.expectedTurnId }
			default: throw Error(`Unexpected method: ${method}`)
		}
	}
	async respond() {}
	connected() { return true }
	subscribe(listener: (event: DevoNativeTransportEvent) => void) { this.listener = listener; return () => { this.listener = undefined } }
	emit(method: string, params: unknown) { this.listener?.({ type: "notification", method, params }) }
}
function View() {
	queue = useComposerQueue(session.id, "/repo")
	return <ComposerStatusStack queueItems={queue.queueItems} onEditQueueItem={queue.editQueueItem} onSteerQueueItem={queue.steerQueueItem} onRemoveQueueItem={queue.removeQueueItem} />
}
async function mount(transport: Transport) {
	client = createDevoClient({ directory: "/repo", transport })
	const subscription = await client.event.subscribe()
	const events: any[] = []
	void (async () => { for await (const event of subscription.stream) { events.push(event.payload); processEvent(event.payload) } })()
	const container = document.createElement("div")
	document.body.append(container)
	root = createRoot(container)
	await act(async () => { root!.render(<Provider store={appStore}><View /></Provider>); await Bun.sleep(10) })
	return events
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	client?.dispose()
	document.body.replaceChildren()
})

for (const [source, label] of Object.entries({ agent: "Agent message", backgroundTask: "Background notification", schedule: "Scheduled notification" })) {
	test(`real SDK ${source} queue keeps source through list/live/snapshot/update/push and guards human actions`, async () => {
		const origin: HostMessageOrigin = { eventId: "event-1", source: source as HostMessageOrigin["source"], sender: "worker-1", delivery: "followUp" }
		const transport = new Transport()
		transport.entry = wireEntry(origin)
		transport.snapshot = true
		const events = await mount(transport)
		const initialUpdates = events.filter(event => event.type === "session.queue.updated")
		expect(initialUpdates).toHaveLength(2) // subscription snapshot, then hook list refresh
		for (const update of initialUpdates) expect(update.properties.entries[0].origin).toEqual(origin)
		await act(async () => {
			const list = await client.session.queue.list({ sessionId: session.id })
			expect(list.data.entries[0].origin).toEqual(origin)
		})
		await act(async () => {
			transport.emit("queue/updated", { sessionId: session.id, queue: [transport.entry], change: "added" })
			await Bun.sleep(5)
		})
		expect(queue.queueItems[0].origin).toEqual(origin)
		expect(document.body.textContent).toContain(`${label} · worker-1`)
		expect(document.body.innerHTML).not.toContain("Edit queued message")
		expect(document.body.textContent).not.toContain("Steer")
		const before = transport.requests.length
		await act(async () => {
			expect(await queue.editQueueItem(queue.queueItems[0])).toBeNull()
			await queue.steerQueueItem(queue.queueItems[0])
		})
		expect(transport.requests.slice(before)).toEqual([])
		await act(async () => {
            transport.emit("queue/updated", { sessionId: session.id, queue: [transport.entry, { ...transport.entry, queueItemId: "queue-2", position: 1 }], change: "added" })
            await Bun.sleep(5)
        })
        await act(async () => { await queue.reorderQueueItem("queue-1", "queue-2") })
		expect(transport.requests.at(-1)).toEqual({ method: "session/queue/update", params: { sessionId: session.id, queueItemId: "queue-1", position: 2 } })
		expect(queue.queueItems[0].origin).toEqual(origin)
		await act(async () => { await queue.removeQueueItem(queue.queueItems.find(item => item.id === "queue-1")!) })
		expect(transport.requests.at(-1)).toEqual({ method: "session/queue/remove", params: { sessionId: session.id, queueItemId: "queue-1" } })
		await act(async () => { await client.session.queue.push({ sessionId: session.id, parts: [{ type: "text", text: "request" }] }); await Bun.sleep(5) })
		expect(queue.queueItems[0].origin).toEqual(origin)
		const updates = events.filter(event => event.type === "session.queue.updated" && event.properties.entries.length)
		// Includes subscription snapshot, list, live event, reorder response, and push response.
		expect(updates.length).toBeGreaterThanOrEqual(5)
		for (const update of updates) expect(update.properties.entries[0].origin).toEqual(origin)
	})
}

test("human queue entries still expose and execute edit/steer", async () => {
	const transport = new Transport()
	await mount(transport)
	expect(queue.queueItems[0].origin).toBeUndefined()
	expect(document.body.innerHTML).toContain("Edit queued message")
	expect(document.body.textContent).toContain("Steer")
	await act(async () => { expect(await queue.editQueueItem(queue.queueItems[0])).toBe("Result ready") })
	expect(transport.requests.at(-1)?.method).toBe("session/queue/remove")
	await act(async () => {
		transport.emit("queue/updated", { sessionId: session.id, queue: [transport.entry], change: "added" })
		transport.emit("turn/started", { sessionId: session.id, turn: { id: "turn-1", sessionId: session.id, status: "inProgress", startedAt: at } })
		await Bun.sleep(5)
	})
	const beforeSteer = transport.requests.length
	await act(async () => { await queue.steerQueueItem(queue.queueItems[0]) })
	expect(transport.requests.slice(beforeSteer).filter(request => request.method !== "session/queue/list")).toEqual([{ method: "session/queue/steer", params: { sessionId: session.id, queueItemId: "queue-1", expectedTurnId: "turn-1" } }])
})
