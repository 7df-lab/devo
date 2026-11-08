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
	listener?: (event: DevoNativeTransportEvent) => void
	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		switch (method) {
			case "initialize": return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			case "session/new": return { session: nativeSession }
			case "subscription/create": return { subscriptionId: "sub-1", cursors: [] }
			case "subscription/ack": return { serverTimeMs: 1 }
			case "session/queue/list": return { entries: [entry] }
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
async function setup() {
	const transport = new QueueTransport()
	const client = createDevoClient({ directory: "/repo", transport })
	await client.session.create()
	transport.listener?.({ type: "notification", method: "turn/started", params: { turn: nativeTurnInProgress } })
	return { client, transport }
}

describe("Native queued steering", () => {
	test("queue list and steer preserve every Native input field", async () => {
		const { client, transport } = await setup()
		try {
			expect((await client.session.queue.list({ sessionId: nativeSession.id })).data.entries).toEqual([entry])
			await client.session.steer({ sessionId: nativeSession.id, queueItemId: entry.queueItemId })
			expect(transport.requests.filter(({ method }) => method === "turn/steer")).toEqual([{
				method: "turn/steer",
				params: {
					sessionId: nativeSession.id,
					expectedTurnId: nativeTurnInProgress.id,
					input: queuedInput,
					idempotencyKey: expect.any(String),
				},
			}])
		} finally { client.dispose() }
	})

	test("steering uses the latest queue notification instead of an earlier preview", async () => {
		const { client, transport } = await setup()
		try {
			await client.session.queue.list({ sessionId: nativeSession.id })
			const updated = { ...entry, input: [{ type: "skill", name: "updated-skill" }] }
			transport.listener?.({ type: "notification", method: "queue/updated", params: {
				sessionId: nativeSession.id, change: "updated", queueItemId: entry.queueItemId, queue: [updated],
			} })
			await client.session.steer({ sessionId: nativeSession.id, queueItemId: entry.queueItemId })
			expect(transport.requests.find(({ method }) => method === "turn/steer")?.params).toEqual({
				sessionId: nativeSession.id, expectedTurnId: nativeTurnInProgress.id,
				input: updated.input, idempotencyKey: expect.any(String),
			})
		} finally { client.dispose() }
	})

	test("a missing queue entry fails before sending or deleting anything", async () => {
		const { client, transport } = await setup()
		try {
			await expect(client.session.steer({ sessionId: nativeSession.id, queueItemId: "missing" }))
				.rejects.toThrow("Queued message is no longer available to steer")
			expect(transport.requests.filter(({ method }) => method === "turn/steer" || method === "session/queue/remove")).toEqual([])
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
