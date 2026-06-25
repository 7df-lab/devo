import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoAcpTransport, type DevoAcpTransportEvent } from "./client"
import type { AcpSessionInfo, AcpSessionNotification } from "./generated"

class FakeTransport implements DevoAcpTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	private listeners: Array<(event: DevoAcpTransportEvent) => void> = []

	constructor(
		private readonly handler: (
			method: string,
			params: unknown,
			directory?: string,
			transport?: FakeTransport,
		) => unknown,
	) {}

	async request(method: string, params?: unknown, directory?: string): Promise<unknown> {
		this.requests.push({ method, params, directory })
		return this.handler(method, params, directory, this)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoAcpTransportEvent) => void): () => void {
		this.listeners.push(listener)
		return () => {
			this.listeners = this.listeners.filter((item) => item !== listener)
		}
	}

	connected(): boolean {
		return true
	}

	emitSessionUpdate(params: unknown): void {
		for (const listener of this.listeners) {
			listener({ type: "notification", method: "session/update", params })
		}
	}
}

const initializeResult = {
	protocolVersion: 1,
	agentCapabilities: {},
	authMethods: [],
}

const storedSession = {
	sessionId: "stored-session",
	cwd: "/stored/repo",
	title: "Stored session",
	updatedAt: "2026-06-24T00:00:00.000Z",
} satisfies AcpSessionInfo

const otherStoredSession = {
	sessionId: "other-stored-session",
	cwd: "/stored/repo",
	title: "Other stored session",
	updatedAt: "2026-06-24T00:00:00.000Z",
} satisfies AcpSessionInfo

async function nextPayload(stream: AsyncIterator<any>, label: string): Promise<any> {
	const result = await Promise.race([
		stream.next(),
		new Promise<IteratorResult<any>>((resolve) =>
			setTimeout(() => resolve({ value: { payload: { type: `timeout:${label}` } }, done: false }), 50),
		),
	])
	return result.value.payload
}

describe("ACP desktop SDK session cwd discovery", () => {
	test("discovers cwd before loading messages for an unknown session", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [storedSession] }
			if (method === "session/load") {
				expect(params).toEqual({
					sessionId: "stored-session",
					cwd: "/stored/repo",
					additionalDirectories: [],
					mcpServers: [],
				})
				return {}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ transport })

		const result = await client.session.messages({ sessionID: "stored-session" })

		expect(result.data).toEqual([])
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"session/list",
			"session/load",
		])
	})

	test("passes history limit through session/load meta and reloads larger windows", async () => {
		const transport = new FakeTransport((method, params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [storedSession] }
			if (method === "session/load") {
				const limit = (params as { _meta?: Record<string, number> })._meta?.["devo/historyLimit"]
				if (limit === 1) {
					tx?.emitSessionUpdate({
						sessionId: "stored-session",
						update: {
							sessionUpdate: "user_message_chunk",
							messageId: "history-1",
							content: { type: "text", text: "new" },
						},
					} satisfies AcpSessionNotification)
				} else if (limit === 2) {
					tx?.emitSessionUpdate({
						sessionId: "stored-session",
						update: {
							sessionUpdate: "user_message_chunk",
							messageId: "history-0",
							content: { type: "text", text: "old" },
						},
					} satisfies AcpSessionNotification)
					tx?.emitSessionUpdate({
						sessionId: "stored-session",
						update: {
							sessionUpdate: "user_message_chunk",
							messageId: "history-1",
							content: { type: "text", text: "new" },
						},
					} satisfies AcpSessionNotification)
				}
				return {}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ transport })

		const first = await client.session.messages({ sessionID: "stored-session", limit: 1 })
		const second = await client.session.messages({ sessionID: "stored-session", limit: 2 })
		const firstAgain = await client.session.messages({ sessionID: "stored-session", limit: 1 })

		expect(first.data.map((message) => message.parts[0]?.text)).toEqual(["new"])
		expect(second.data.map((message) => message.parts[0]?.text)).toEqual(["old", "new"])
		expect(firstAgain.data.map((message) => message.parts[0]?.text)).toEqual(["new"])
		expect(transport.requests.filter((request) => request.method === "session/load")).toEqual([
			{
				method: "session/load",
				directory: undefined,
				params: {
					sessionId: "stored-session",
					cwd: "/stored/repo",
					additionalDirectories: [],
					mcpServers: [],
					_meta: { "devo/historyLimit": 1 },
				},
			},
			{
				method: "session/load",
				directory: undefined,
				params: {
					sessionId: "stored-session",
					cwd: "/stored/repo",
					additionalDirectories: [],
					mcpServers: [],
					_meta: { "devo/historyLimit": 2 },
				},
			},
		])
	})

	test("keeps server-expanded limited history windows intact", async () => {
		const transport = new FakeTransport((method, params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [storedSession] }
			if (method === "session/load") {
				expect((params as { _meta?: Record<string, number> })._meta).toEqual({
					"devo/historyLimit": 1,
				})
				tx?.emitSessionUpdate({
					sessionId: "stored-session",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "history-0",
						content: { type: "text", text: "user" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "stored-session",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "history-1",
						content: { type: "text", text: "assistant" },
					},
				} satisfies AcpSessionNotification)
				return {}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ transport })

		const result = await client.session.messages({ sessionID: "stored-session", limit: 1 })

		expect(result.data.map((message) => message.parts[0]?.text)).toEqual(["user", "assistant"])
	})

	test("does not synthesize a default cwd for unknown session updates", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [storedSession] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emitSessionUpdate({
			sessionId: "stored-session",
			update: {
				sessionUpdate: "session_info_update",
				title: "Stored session renamed",
				updatedAt: "2026-06-24T00:01:00.000Z",
				_meta: {
					"devo/session": {
						created_at: "2026-06-24T00:00:00.000Z",
						updated_at: "2026-06-24T00:01:00.000Z",
						last_activity_at: "2026-06-24T00:00:00.000Z",
					},
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayload(stream, "session-info")).toEqual({
			type: "session.updated",
			properties: {
				info: {
					id: "stored-session",
					title: "Stored session renamed",
					directory: "/stored/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T00:01:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
				session: {
					id: "stored-session",
					title: "Stored session renamed",
					directory: "/stored/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T00:01:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
			},
		})
	expect(transport.requests.map((request) => request.method)).toEqual([
		"initialize",
		"session/list",
	])
	})

	test("keeps cached parts scoped when loaded sessions reuse message IDs", async () => {
		const transport = new FakeTransport((method, params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [storedSession, otherStoredSession] }
			if (method === "session/load") {
				const sessionId = (params as { sessionId: string }).sessionId
				tx?.emitSessionUpdate({
					sessionId,
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "shared-message",
						content: {
							type: "text",
							text: sessionId === "stored-session" ? "first session" : "second session",
						},
					},
				} satisfies AcpSessionNotification)
				return {}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ transport })

		const first = await client.session.messages({ sessionID: "stored-session" })
		const second = await client.session.messages({ sessionID: "other-stored-session" })
		const firstAgain = await client.session.messages({ sessionID: "stored-session" })

		expect(first.data[0].parts).toEqual([
			{
				id: "shared-message-text",
				sessionID: "stored-session",
				messageID: "shared-message",
				type: "text",
				text: "first session",
				time: { start: first.data[0].parts[0].time.start },
			},
		])
		expect(second.data[0].parts).toEqual([
			{
				id: "shared-message-text",
				sessionID: "other-stored-session",
				messageID: "shared-message",
				type: "text",
				text: "second session",
				time: { start: second.data[0].parts[0].time.start },
			},
		])
		expect(firstAgain.data).toEqual(first.data)
	})
})
