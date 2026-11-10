import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"

const nativeSession = {
	id: "session-1",
	version: 1,
	cwd: "/repo",
	title: "Native session",
	parent: null,
	createdAt: "2026-08-22T00:00:00Z",
	lastActivityAt: "2026-08-22T00:00:00Z",
	status: "idle",
	flags: [],
	archived: false,
	ephemeral: false,
	model: { provider: "test", model: "test-model" },
	settings: { permissionProfile: "default" },
	preview: "",
	queuedCount: 0,
	usage: {
		total: {
			inputTokens: 0,
			outputTokens: 0,
			cacheCreationInputTokens: 0,
			cacheReadInputTokens: 0,
			reasoningTokens: 0,
			totalTokens: 0,
			callCount: 0,
			meteredCallCount: 0,
			failedCallCount: 0,
			cancelledCallCount: 0,
		},
		byPurpose: [],
		updatedAt: "2026-08-22T00:00:00Z",
	},
}

const nativeTurnInProgress = {
	id: "turn-1",
	sessionId: nativeSession.id,
	sequence: 1,
	kind: "regular",
	status: "inProgress",
	model: nativeSession.model,
	startedAt: "2026-08-24T00:00:00Z",
}


const queuedInput = [
	{ type: "text", text: "Review these references" },
	{ type: "image", uri: "data:image/png;base64,aGVsbG8=", mimeType: "image/png", detail: "high" },
	{ type: "localImage", path: "/repo/screenshot.png", detail: "auto" },
	{ type: "audio", uri: "data:audio/wav;base64,aGVsbG8=", mimeType: "audio/wav" },
	{ type: "skill", name: "review" },
	{ type: "mention", uri: "file:///repo/notes.md" },
]
const entry = {
	queueItemId: "queue-1",
	position: 1,
	preview: "Review these references",
	enqueuedAt: "2026-08-24T00:00:00Z",
	input: queuedInput,
}
class QueueTransport implements DevoNativeTransport {
	requests: Array<{ method: string; params: unknown }> = []
	failSteer = false
	keptQueued = false
	keptEntry: typeof entry & { recoveryRequired?: boolean } = { ...entry, recoveryRequired: true }
	retried = false
	held = false
	steerGate?: Promise<void>
	failList = false
	listGate?: Promise<void>
	onList?: () => void
	mutationGate?: Promise<void>
	onMutation?: () => void
	listener?: (event: DevoNativeTransportEvent) => void
	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		switch (method) {
			case "initialize": return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			case "session/new": return { session: nativeSession }
			case "subscription/create": return { subscriptionId: "sub-1", cursors: [] }
			case "subscription/ack": return { serverTimeMs: 1 }
			case "session/queue/list": {
				if (this.failList) throw new Error("Queue refresh unavailable")
				const entries = [{ ...entry, ...(this.held ? { recoveryRequired: true } : {}), queueItemId: this.retried ? "queue-fresh" : entry.queueItemId }]
				this.onList?.()
				await this.listGate
				return { entries }
			}
			case "session/queue/update":
			case "session/queue/push":
				this.onMutation?.()
				await this.mutationGate
				return method === "session/queue/push" ? { outcome: "queued", entry } : { entry }
			case "session/queue/steer":
				await this.steerGate
				if (this.failSteer) throw new Error("Unsupported or response lost")
				return this.keptQueued ? { outcome: "keptQueued", entry: this.keptEntry } : { outcome: "accepted", queueItemId: entry.queueItemId, turnId: "turn-1" }
			case "session/queue/retry": this.retried = true; return {}
			case "turn/steer": return { outcome: "injected", itemId: "steered-1" }
			default: throw new Error(`Unexpected method: ${method}`)
		}
	}
	async respond() {}
	subscribe(listener: (event: DevoNativeTransportEvent) => void) {
		this.listener = listener
		return () => { this.listener = undefined }
	}
	connected() { return true }
}
async function setup(active = true) {
	const transport = new QueueTransport()
	const client = createDevoClient({ directory: "/repo", transport })
	await client.session.create()
	if (active) transport.listener?.({ type: "notification", method: "turn/started", params: { turn: nativeTurnInProgress } })
	return { client, transport }
}

describe("Native queued steering", () => {
	test("steers atomically by identity without resubmitting Native attachments", async () => {
		const { client, transport } = await setup()
		try {
			await client.session.queue.list({ sessionId: nativeSession.id })
			const params = { sessionId: nativeSession.id, queueItemId: entry.queueItemId, expectedTurnId: "turn-1" }
			expect((await client.session.queue.steer(params)).data).toEqual({ outcome: "accepted", queueItemId: "queue-1", turnId: "turn-1" })
			await client.session.queue.steer(params)
			expect(transport.requests.filter(({ method }) => method.includes("steer") || method.includes("remove"))).toEqual([
				{ method: "session/queue/steer", params }, { method: "session/queue/steer", params },
			])
		} finally { client.dispose() }
	})

	test("empty caches and a fresh client do not prevent receipt retries", async () => {
		for (let attempt = 0; attempt < 2; attempt++) {
			const { client, transport } = await setup()
			try {
				await client.session.steer({ sessionId: nativeSession.id, queueItemId: "queue-1" })
				expect(transport.requests.find(({ method }) => method === "session/queue/steer")).toEqual({ method: "session/queue/steer", params: { sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "turn-1" } })
			} finally { client.dispose() }
		}
	})

	test("receipt recovery works without an active turn cache", async () => {
		const { client, transport } = await setup(false)
		try {
			await client.session.steer({ sessionId: nativeSession.id, queueItemId: "queue-1" })
			expect(transport.requests.find(({ method }) => method === "session/queue/steer")).toEqual({ method: "session/queue/steer", params: { sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "" } })
		} finally { client.dispose() }
	})

	test("a delayed accepted response cannot erase a newer held notification", async () => {
		const { client, transport } = await setup()
		const { stream } = await client.event.subscribe()
		try {
			let release!: () => void
			transport.steerGate = new Promise((resolve) => { release = resolve })
			const pending = client.session.queue.steer({ sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "turn-1" })
			transport.held = true
			const held = { ...entry, recoveryRequired: true }
			transport.listener?.({ type: "notification", method: "queue/updated", params: {
				sessionId: nativeSession.id, change: "updated", queueItemId: entry.queueItemId, queue: [held],
			} })
			release()
			await pending
			client.dispose()
			const queues = []
			for await (const event of stream) {
				if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
			}
			expect(queues).toEqual([[held], [held]])
		} finally { client.dispose() }
	})

	test("an old acceptance receipt preserves held input when refresh fails", async () => {
		const { client, transport } = await setup(false)
		const { stream } = await client.event.subscribe()
		try {
			transport.held = true
			const held = { ...entry, recoveryRequired: true }
			await client.session.queue.list({ sessionId: nativeSession.id })
			transport.failList = true
			expect((await client.session.steer({ sessionId: nativeSession.id, queueItemId: "queue-1" })).data.outcome).toBe("accepted")
			client.dispose()
			const queues = []
			for await (const event of stream) {
				if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
			}
			expect(queues).toEqual([[held]])
			expect(transport.requests.filter(({ method }) => method === "session/queue/list").length).toBe(2)
		} finally { client.dispose() }
	})

	test("a stale keptQueued response cannot downgrade a newer held notification", async () => {
		const { client, transport } = await setup()
		const { stream } = await client.event.subscribe()
		try {
			let release!: () => void
			transport.keptQueued = true
			transport.keptEntry = entry
			transport.steerGate = new Promise((resolve) => { release = resolve })
			const pending = client.session.queue.steer({ sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "stale" })
			transport.held = true
			const held = { ...entry, recoveryRequired: true }
			transport.listener?.({ type: "notification", method: "queue/updated", params: {
				sessionId: nativeSession.id, change: "updated", queueItemId: entry.queueItemId, queue: [held],
			} })
			release()
			await pending
			client.dispose()
			const queues = []
			for await (const event of stream) {
				if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
			}
			expect(queues).toEqual([[held], [held]])
		} finally { client.dispose() }
	})

	test("a delayed list response remains available to its caller without hiding a newer held notification", async () => {
		const { client, transport } = await setup()
		const { stream } = await client.event.subscribe()
		try {
			let release!: () => void
			transport.listGate = new Promise((resolve) => { release = resolve })
			const started = new Promise<void>((resolve) => { transport.onList = resolve })
			const pending = client.session.queue.list({ sessionId: nativeSession.id })
			await started
			const held = { ...entry, recoveryRequired: true }
			transport.listener?.({ type: "notification", method: "queue/updated", params: {
				sessionId: nativeSession.id, change: "updated", queueItemId: entry.queueItemId, queue: [held],
			} })
			release()
			expect((await pending).data.entries).toEqual([entry])
			client.dispose()
			const queues = []
			for await (const event of stream) {
				if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
			}
			expect(queues).toEqual([[held]])
		} finally { client.dispose() }
	})

	test("an older list cannot overwrite a newer completed list", async () => {
		const { client, transport } = await setup()
		const { stream } = await client.event.subscribe()
		try {
			let release!: () => void
			transport.listGate = new Promise((resolve) => { release = resolve })
			const started = new Promise<void>((resolve) => { transport.onList = resolve })
			const older = client.session.queue.list({ sessionId: nativeSession.id })
			await started
			transport.listGate = undefined
			transport.held = true
			await client.session.queue.list({ sessionId: nativeSession.id })
			release()
			expect((await older).data.entries).toEqual([entry])
			client.dispose()
			const queues = []
			for await (const event of stream) {
				if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
			}
			expect(queues).toEqual([[{ ...entry, recoveryRequired: true }]])
		} finally { client.dispose() }
	})

	for (const operation of ["update", "push"] as const) {
		for (const change of ["promoted", "drained", "removed", "updated"] as const) {
			test(`a delayed queue ${operation} response preserves newer ${change} state`, async () => {
				const { client, transport } = await setup()
				const { stream } = await client.event.subscribe()
				try {
					let release!: () => void
					transport.mutationGate = new Promise((resolve) => { release = resolve })
					const started = new Promise<void>((resolve) => { transport.onMutation = resolve })
					const pending = operation === "update"
						? client.session.queue.update({ sessionId: nativeSession.id, queueItemId: "queue-1", parts: [{ type: "text", text: "edited" }] })
						: client.session.queue.push({ sessionId: nativeSession.id, parts: [{ type: "text", text: "queued" }] })
					await started
					const authoritative = change === "updated" ? [{ ...entry, recoveryRequired: true }] : []
					transport.listener?.({ type: "notification", method: "queue/updated", params: {
						sessionId: nativeSession.id, change, queueItemId: entry.queueItemId, queue: authoritative,
					} })
					release()
					expect((await pending).data).toEqual(operation === "update" ? { entry } : { outcome: "queued", queueItemId: entry.queueItemId })
					client.dispose()
					const queues = []
					for await (const event of stream) {
						if (event.payload.type === "session.queue.updated") queues.push(event.payload.properties.entries)
					}
					expect(queues).toEqual([authoritative])
				} finally { client.dispose() }
			})
		}
	}

	test("unsupported or lost responses never fall back to raw steer or removal", async () => {
		const { client, transport } = await setup()
		try {
			transport.failSteer = true
			const params = { sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "turn-1" }
			await expect(client.session.queue.steer(params)).rejects.toThrow()
			transport.failSteer = false
			await client.session.queue.steer(params)
			expect(transport.requests.filter(({ method }) => method === "turn/steer" || method === "session/queue/remove")).toEqual([])
		} finally { client.dispose() }
	})

	test("keptQueued preserves recovery state and explicit retry uses a fresh identity", async () => {
		const { client, transport } = await setup()
		try {
			transport.keptQueued = true
			const result = await client.session.queue.steer({ sessionId: nativeSession.id, queueItemId: "queue-1", expectedTurnId: "stale-turn" })
			expect(result.data).toEqual({ outcome: "keptQueued", entry: { ...entry, recoveryRequired: true } })
			await client.session.queue.retry({ sessionId: nativeSession.id, queueItemId: "queue-1" })
			expect((await client.session.queue.list({ sessionId: nativeSession.id })).data.entries[0].queueItemId).toBe("queue-fresh")
			expect(transport.requests.find(({ method }) => method === "session/queue/retry")).toEqual({ method: "session/queue/retry", params: { sessionId: nativeSession.id, queueItemId: "queue-1" } })
		} finally { client.dispose() }
	})

	test("existing direct-text steering keeps its request shape", async () => {
		const { client, transport } = await setup()
		try {
			await client.session.steer({ sessionId: nativeSession.id, parts: [{ type: "text", text: "Direct steer" }] })
			expect(transport.requests.find(({ method }) => method === "turn/steer")?.params).toEqual({
				sessionId: nativeSession.id, expectedTurnId: nativeTurnInProgress.id,
				input: [{ type: "text", text: "Direct steer" }], idempotencyKey: expect.any(String),
			})
		} finally { client.dispose() }
	})
})


test("queue recovery preserves host provenance and complete Native input together", async () => {
 const { client, transport } = await setup()
 try {
  const origin = { eventId: "host-event", source: "agent" as const, sender: "child", delivery: "followUp" as const }
  transport.keptQueued = true
  transport.keptEntry = { ...entry, recoveryRequired: true, ...{ origin } }
  const result = await client.session.queue.steer({ sessionId: nativeSession.id, queueItemId: entry.queueItemId, expectedTurnId: "stale" })
  expect(result.data).toEqual({ outcome: "keptQueued", entry: { ...entry, recoveryRequired: true, origin } })
 } finally { client.dispose() }
})
