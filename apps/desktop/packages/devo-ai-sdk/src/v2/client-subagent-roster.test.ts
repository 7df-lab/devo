import { describe, expect, test } from "bun:test"
import {
	createDevoClient,
	type DevoNativeTransport,
	type DevoNativeTransportEvent,
	type SessionRosterChange,
} from "./client"

const child = {
	id: "child-1",
	version: 1,
	cwd: "/repo",
	title: "Review tests",
	parent: { kind: "agent", sessionId: "parent-1" },
	createdAt: "2026-11-08T00:00:00Z",
	lastActivityAt: "2026-11-08T00:00:00Z",
	status: "idle",
	flags: ["waitingApproval"],
	archived: false,
	ephemeral: false,
	activity: "working",
	model: { provider: "test", model: "test" },
	settings: { permissionProfile: "default" },
	preview: "",
	queuedCount: 1,
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
		updatedAt: "2026-11-08T00:00:00Z",
	},
}

class Transport implements DevoNativeTransport {
	requests: Array<{ method: string; params: unknown }> = []
	error: Error | undefined
	listeners = new Set<(event: DevoNativeTransportEvent) => void>()
	emit(event: DevoNativeTransportEvent) {
		for (const listener of this.listeners) listener(event)
	}
	async request(method: string, params?: unknown) {
		this.requests.push({ method, params })
		if (method === "initialize")
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		if (this.error) throw this.error
		if (method === "subscription/create")
			return { subscriptionId: "roster", snapshots: [], cursors: [] }
		if (method === "session/read") return { session: child }
		if (method === "session/list") {
			return (params as { includeChildren?: boolean }).includeChildren
				? { data: [child], nextCursor: "200" }
				: { data: [], nextCursor: null }
		}
		throw new Error(`Unexpected ${method}`)
	}
	async respond() {}
	subscribe(listener: (event: DevoNativeTransportEvent) => void) {
		this.listeners.add(listener)
		return () => {
			this.listeners.delete(listener)
		}
	}
	connected() {
		return true
	}
}

describe("read-only child session discovery", () => {
	test("roster forwards pagination and preserves Native flags without resuming or loading history", async () => {
		const transport = new Transport()
		const client = createDevoClient({ directory: "/repo", transport })
		try {
			const params = {
				cwds: [],
				includeChildren: true,
				limit: 200,
				cursor: "100",
			}
			expect(await client.session.roster(params)).toEqual({
				data: [child],
				nextCursor: "200",
			})
			expect(transport.requests.filter((r) => r.method !== "initialize")).toEqual([
				{ method: "session/list", params },
			])
		} finally {
			client.dispose()
		}
	})
	test("cold child lookup reads its exact ID instead of the root-only roster", async () => {
		const transport = new Transport()
		const client = createDevoClient({ transport })
		try {
			const [first, second] = await Promise.all([
				client.session.get({ sessionId: child.id }),
				client.session.get({ sessionId: child.id }),
			])
			expect(first).toEqual(second)
			expect(first.data).toMatchObject({
				id: child.id,
				parentId: "parent-1",
				directory: "/repo",
			})
			expect(transport.requests.filter((r) => r.method !== "initialize")).toEqual([
				{ method: "session/read", params: { sessionId: child.id } },
			])
		} finally {
			client.dispose()
		}
	})
	test("missing children return undefined; connection errors are not disguised as missing", async () => {
		const transport = new Transport()
		const client = createDevoClient({ transport })
		try {
			transport.error = Object.assign(new Error("gone"), {
				code: "SessionNotFound",
			})
			expect(await client.session.get({ sessionId: "missing" })).toEqual({
				data: undefined,
			})
			transport.error = new Error("connection lost")
			await expect(client.session.get({ sessionId: "missing" })).rejects.toThrow("connection lost")
		} finally {
			client.dispose()
		}
	})
})

test("passive roster observers do not consume the main event stream or start subscriptions", async () => {
	const transport = new Transport()
	const client = createDevoClient({ directory: "/repo", transport })
	const observed: SessionRosterChange[] = []
	const second: SessionRosterChange[] = []
	const stop = client.session.observeRoster((change: SessionRosterChange) => observed.push(change))
	client.session.observeRoster((change: SessionRosterChange) => second.push(change))
	expect(transport.requests).toEqual([])
	await client.session.roster({ includeChildren: true })
	const { stream } = await client.global.event()
	const iterator = stream[Symbol.asyncIterator]()
	const pending = iterator.next()
	const before = transport.requests.length
	transport.emit({
		type: "notification",
		method: "session/statusChanged",
		params: {
			sessionId: child.id,
			status: "idle",
			flags: ["waitingApproval"],
			activity: "working",
			activeTurnId: null,
		},
	})
	expect(observed).toEqual([
		{
			type: "patch",
			sessionId: child.id,
			patch: {
				status: "idle",
				flags: ["waitingApproval"],
				activity: "working",
				activeTurnId: null,
			},
		},
	])
	expect(second).toEqual(observed)
	expect((await pending).value?.payload).toEqual({
		type: "session.status",
		properties: {
			sessionId: child.id,
			status: { type: "idle" },
		},
	})
	expect(transport.requests.length).toBe(before)
	stop()
	transport.emit({ type: "closed" })
	expect(observed).toHaveLength(1)
	expect(second.at(-1)).toEqual({ type: "disconnected" })
	client.dispose()
	transport.emit({ type: "closed" })
	expect(second).toHaveLength(2)
	expect(transport.listeners.size).toBe(0)
})

test("validated Native creation and metadata reach roster observers unchanged", async () => {
	const transport = new Transport()
	const client = createDevoClient({ directory: "/repo", transport })
	const observed: SessionRosterChange[] = []
	client.session.observeRoster((change: SessionRosterChange) => observed.push(change))
	await client.session.roster({ includeChildren: true })
	const updated = { ...child, title: "Renamed", flags: [], queuedCount: 0 }
	transport.emit({ type: "notification", method: "session/created", params: { session: child } })
	transport.emit({
		type: "notification",
		method: "session/metadataUpdated",
		params: { session: updated },
	})
	expect(observed).toEqual([
		{ type: "upsert", session: child },
		{ type: "upsert", session: updated },
	])
	client.dispose()
})
