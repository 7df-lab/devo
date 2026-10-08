import { describe, expect, test } from "bun:test"
import {
	createDevoClient,
	isSessionNotFoundError,
	type DevoNativeTransport,
	type DevoNativeTransportEvent,
} from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown }> = []
	private listener?: (event: DevoNativeTransportEvent) => void

	constructor(private readonly handler: (method: string, params: unknown) => unknown) {}

	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		return this.handler(method, params)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listener = listener
		return () => {
			this.listener = undefined
		}
	}

	emit(event: DevoNativeTransportEvent): void {
		this.listener?.(event)
	}

	connected(): boolean {
		return true
	}
}

const nativeSession = {
	id: "missing-session",
	version: 1,
	cwd: "/repo",
	title: "Missing",
	parent: null,
	createdAt: "2026-01-01T00:00:00.000Z",
	lastActivityAt: "2026-01-01T00:00:00.000Z",
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
		updatedAt: "2026-01-01T00:00:00.000Z",
	},
}

describe("isSessionNotFoundError", () => {
	test("matches server message and SessionNotFound code", () => {
		expect(isSessionNotFoundError(new Error("session does not exist"))).toBe(true)
		const coded = new Error("gone") as Error & { code?: string }
		coded.code = "SessionNotFound"
		expect(isSessionNotFoundError(coded)).toBe(true)
		expect(isSessionNotFoundError(new Error("timeout"))).toBe(false)
	})
})

describe("session.messages soft-handles missing sessions", () => {
	test("returns empty messages and emits session.deleted when resume fails", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			if (method === "session/resume") {
				const error = new Error("session does not exist") as Error & { code?: string }
				error.code = "SessionNotFound"
				throw error
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const deletedIds: string[] = []
		const subscription = await client.event.subscribe()
		const consumer = (async () => {
			for await (const globalEvent of subscription.stream) {
				if (globalEvent.payload?.type === "session.deleted") {
					deletedIds.push(String(globalEvent.payload.properties?.info?.id ?? ""))
					break
				}
			}
		})()

		const result = await client.session.messages({ sessionId: "missing-session" })
		expect(result.data).toEqual([])
		await consumer
		expect(deletedIds).toEqual(["missing-session"])
		expect(transport.requests.some((request) => request.method === "session/resume")).toBe(true)
	})
})

describe("session.queue.list resumes cold historical sessions", () => {
	test("loads via session/resume before queue/list", async () => {
		let resumed = false
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "session/resume") {
				resumed = true
				return { session: nativeSession }
			}
			if (method === "session/items/list") {
				return { data: [], nextCursor: null }
			}
			if (method === "session/queue/list") {
				if (!resumed) {
					const error = new Error("session does not exist") as Error & { code?: string }
					error.code = "SessionNotFound"
					throw error
				}
				return {
					entries: [
						{
							queueItemId: "q1",
							position: 0,
							preview: "hello",
							input: [{ type: "text", text: "hello" }],
							enqueuedAt: "2026-01-01T00:00:00.000Z",
						},
					],
				}
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.session.queue.list({ sessionId: "missing-session" })
		expect(resumed).toBe(true)
		expect(result.data.entries).toHaveLength(1)
		expect(result.data.entries[0]?.queueItemId).toBe("q1")
		const methods = transport.requests.map((request) => request.method)
		expect(methods.indexOf("session/resume")).toBeLessThan(methods.indexOf("session/queue/list"))
	})

	test("returns empty entries when the session is truly gone", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "session/resume") {
				const error = new Error("session does not exist") as Error & { code?: string }
				error.code = "SessionNotFound"
				throw error
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.session.queue.list({ sessionId: "missing-session" })
		expect(result.data.entries).toEqual([])
	})
})


describe("runtime restart", () => {
	for (const operation of ["queue list", "send prompt", "evicted actor"] as const) {
		test(`${operation} resumes a previously loaded session after transport closes`, async () => {
			let resumed = false
			let restart = false
			const transport = new FakeTransport((method) => {
				if (method === "initialize") return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
				if (method === "session/list") return { data: [nativeSession], nextCursor: null }
				if (method === "session/resume") {
					resumed = true
					return { session: nativeSession }
				}
				if (method === "session/items/list") return { data: [], nextCursor: null }
				if (method === "subscription/create") return { subscriptionId: restart ? "sub-new" : "sub-old", snapshots: [], replay: [], cursors: [] }
				if (method === "session/queue/list" || method === "session/queue/push") {
					if (!resumed) throw new Error("session does not exist")
					return method === "session/queue/list" ? { entries: [] } : { outcome: "started", turn: { id: "recovered-turn", sessionId: nativeSession.id, sequence: 1, kind: "regular", status: "inProgress", model: nativeSession.model, startedAt: nativeSession.createdAt } }
				}
				throw new Error(`unexpected method ${method}`)
			})
			const client = createDevoClient({ directory: "/repo", transport })
			await client.session.messages({ sessionId: nativeSession.id })
			resumed = false
			restart = true
			const beforeRestart = transport.requests.length
			if (operation !== "evicted actor") transport.emit({ type: "closed" })
			if (operation === "queue list") {
				expect(await client.session.queue.list({ sessionId: nativeSession.id })).toEqual({ data: { entries: [] } })
			} else {
				expect(await client.session.promptAsync({ sessionId: nativeSession.id, parts: [{ type: "text", text: "after restart" }] })).toEqual({ data: { outcome: "started", turnId: "recovered-turn" } })
			}
			expect(transport.requests.slice(beforeRestart).map(({ method }) => method)).toEqual(
				operation === "evicted actor"
					? ["session/queue/push", "session/resume", "subscription/create", "session/queue/push"]
					: ["initialize", "session/resume", "subscription/create",
						operation === "queue list" ? "session/queue/list" : "session/queue/push"],
			)
			if (operation === "evicted actor") {
				expect(transport.requests.filter(({ method }) => method === "session/queue/push")
					.map(({ params }) => params)).toEqual([
					transport.requests.at(-1)?.params, transport.requests.at(-1)?.params,
				])
			}
			client.dispose()
		})
	}
})
