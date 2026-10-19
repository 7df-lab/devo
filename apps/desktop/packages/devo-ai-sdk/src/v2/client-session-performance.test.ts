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

class HistoryTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: any }> = []
	readonly listeners = new Set<(event: DevoNativeTransportEvent) => void>()
	readonly history = Array.from({ length: 2400 }, (_, index) => ({
		id: `item-${index + 1}`, sessionId: nativeSession.id, turnId: "turn-1",
		seq: index + 1, revision: 1, createdAt: nativeSession.createdAt, updatedAt: nativeSession.createdAt,
		state: "completed", item: { type: "assistantMessage", text: `Message ${index + 1}` },
	}))
	firstPage?: Promise<void>
	failPage = false
	recovery: unknown[] = []
	async request(method: string, params?: any): Promise<unknown> {
		this.requests.push({ method, params })
		switch (method) {
			case "initialize": return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			case "session/list": return { data: [nativeSession], nextCursor: null }
			case "session/resume": return { session: nativeSession }
			case "session/queue/list": return { entries: [] }
			case "subscription/create": return {
				subscriptionId: `sub-${this.requests.length}`, cursors: [], replay: [], pendingControlRequests: [],
				recoverySnapshots: params.selectors[0].kind === "session" ? this.recovery : [],
				snapshots: params.selectors[0].kind === "sessionsByCwd" ? [{ streamId: "cwd:/repo", barrierSeq: 0, data: { kind: "sessionsList", sessions: Array.from({ length: 120 }, (_, i) => ({ ...nativeSession, id: `session-${i + 1}` })) } }] : [],
			}
			case "session/items/list": {
				await this.firstPage
				if (this.failPage) throw new Error("temporary history read failure")
				const end = params.cursor === "tail" ? this.history.length : Number(params.cursor.slice(7)) - 1
				const start = Math.max(0, end - params.limit)
				return { data: this.history.slice(start, end), nextCursor: start ? `before:${start + 1}` : null }
			}
			default: throw new Error(`unexpected request ${method}`)
		}
	}
	async respond(): Promise<void> {}
	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.add(listener)
		return () => { this.listeners.delete(listener) }
	}
	connected(): boolean { return true }
	emit(event: DevoNativeTransportEvent): void { for (const listener of this.listeners) listener(event) }
	pages(): unknown[] { return this.requests.filter(r => r.method === "session/items/list").map(r => r.params) }
}

const tailPage = { sessionId: nativeSession.id, cursor: "tail", limit: 160 }
const olderPage = { sessionId: nativeSession.id, cursor: "before:2241", limit: 100 }

describe("bounded Native session loading", () => {
	test("identical session-list reads share one request", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			const [first, second] = await Promise.all([client.session.list({ limit: 5 }), client.session.list({ limit: 5 })])
			expect(first).toEqual(second)
			expect(transport.requests.filter(r => r.method === "session/list")).toHaveLength(1)
		} finally { client.dispose() }
	})

	test("rosters preserve canonical selections already restored by resume", async () => {
		const transport = new HistoryTransport()
		const request = transport.request.bind(transport)
		transport.request = async (method, params) => {
			if (method === "session/resume") return { session: { ...nativeSession, model: { provider: "codex", model: "qa-model" }, settings: { permissionProfile: "default", reasoningEffort: "high" } } }
			return request(method, params)
		}
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			await client.session.queue.list({ sessionId: nativeSession.id })
			const restored = await client.session.get({ sessionId: nativeSession.id })
			await client.event.subscribe()
			expect(await client.session.get({ sessionId: nativeSession.id })).toEqual(restored)
		} finally { client.dispose() }
	})
	test("superseding a turn invalidates the tail window", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			transport.history.splice(-200)
			transport.emit({ type: "notification", method: "turn/superseded", params: { sessionId: nativeSession.id, supersededTurnId: "turn-1" } })
			const result = await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			expect(result.data.map(m => m.info.id)).toEqual(transport.history.slice(-160).map(m => m.id))
			expect(transport.pages()).toEqual([tailPage, tailPage])
		} finally { client.dispose() }
	})
	test("120 idle sessions need one roster and no transcripts or actors", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			await Promise.all([client.event.subscribe(), client.event.subscribe()])
			expect(transport.requests).toEqual([
				{ method: "initialize", params: expect.anything() },
				{ method: "subscription/create", params: { selectors: [{ kind: "sessionsByCwd", cwd: "/repo" }], includeSnapshot: true, replay: "snapshotOnly", after: [] } },
			])
			transport.emit({ type: "notification", method: "session/statusChanged", params: { sessionId: "session-50", status: "active" } })
			await Bun.sleep(0)
			expect(transport.requests.slice(2)).toEqual([
				{ method: "subscription/create", params: { selectors: [{ kind: "session", sessionId: "session-50" }], includeSnapshot: true, replay: "snapshotOnly", after: [] } },
			])
		} finally { client.dispose() }
	})
	test("queue readiness skips history; expanding downloads only missing rows", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			await client.session.queue.list({ sessionId: nativeSession.id })
			expect(transport.pages()).toEqual([])
			const first = await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			expect(first.data.map(m => m.info.id)).toEqual(transport.history.slice(-160).map(m => m.id))
			const expanded = await client.session.messages({ sessionId: nativeSession.id, limit: 260 })
			expect(expanded.data.map(m => m.info.id)).toEqual(transport.history.slice(-260).map(m => m.id))
			await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			await client.session.queue.list({ sessionId: nativeSession.id })
			expect(transport.pages()).toEqual([tailPage, olderPage])
			expect(transport.requests.filter(r => r.method === "session/resume")).toHaveLength(1)
		} finally { client.dispose() }
	})
	test("concurrent wider reads join the first page then fetch the remaining window", async () => {
		const transport = new HistoryTransport()
		let release!: () => void
		transport.firstPage = new Promise<void>(resolve => { release = resolve })
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			const first = client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			const second = client.session.messages({ sessionId: nativeSession.id, limit: 260 })
			await Bun.sleep(0)
			release()
			expect((await first).data).toHaveLength(160)
			expect((await second).data).toHaveLength(260)
			expect(transport.pages()).toEqual([tailPage, olderPage])
		} finally { client.dispose() }
	})
	test("failed reads retry and reconnect invalidates the history cursor", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			transport.failPage = true
			await expect(client.session.messages({ sessionId: nativeSession.id, limit: 160 })).rejects.toThrow("temporary history read failure")
			transport.failPage = false
			await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			transport.emit({ type: "closed" })
			await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			expect(transport.pages()).toEqual([tailPage, tailPage, tailPage])
			expect(transport.requests.filter(r => r.method === "session/resume")).toHaveLength(2)
		} finally { client.dispose() }
	})
	test("full history remains available and exhausted cursors are cached", async () => {
		const transport = new HistoryTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			expect((await client.session.messages({ sessionId: nativeSession.id })).data).toHaveLength(2400)
			expect(transport.pages()).toHaveLength(12)
			expect((await client.session.messages({ sessionId: nativeSession.id, limit: 3000 })).data).toHaveLength(2400)
			expect(transport.pages()).toHaveLength(12)
		} finally { client.dispose() }
	})
	test("snapshot bootstrap restores in-flight text before history arrives", async () => {
		const transport = new HistoryTransport()
		const live = { ...transport.history.at(-1)!, id: "live-item", seq: 2401, state: "running", item: { type: "assistantMessage", text: "Already streamed" } }
		transport.recovery = [{ item: live, accumulated: [] }]
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			const result = await client.session.messages({ sessionId: nativeSession.id, limit: 160 })
			expect(result.data.at(-1)).toEqual({ info: live })
			expect(transport.pages()).toHaveLength(1)
		} finally { client.dispose() }
	})
})
