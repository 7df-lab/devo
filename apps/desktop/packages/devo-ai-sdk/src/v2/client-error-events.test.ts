import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoAcpTransport, type DevoAcpTransportEvent } from "./client"
import type { AcpSessionNotification } from "./generated"

class ErrorEventTransport implements DevoAcpTransport {
	private listeners: Array<(event: DevoAcpTransportEvent) => void> = []

	async request(method: string): Promise<unknown> {
		if (method === "initialize") {
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		}
		if (method === "session/list") {
			return {
				sessions: [
					{
						sessionId: "s1",
						cwd: "/repo",
						title: "Existing session",
						updatedAt: "2026-07-13T00:00:00.000Z",
					},
				],
			}
		}
		throw new Error(`unexpected request ${method}`)
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

	emitOriginalEvent(method: string, originalEvent: Record<string, unknown>): void {
		const params = {
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": method,
				"devo/originalEvent": originalEvent,
			},
		} satisfies AcpSessionNotification
		for (const listener of this.listeners) {
			listener({ type: "notification", method: "session/update", params })
		}
	}
}

async function nextPayload(stream: AsyncIterator<any>): Promise<any> {
	return (
		await Promise.race([
			stream.next(),
			new Promise<IteratorResult<any>>((resolve) =>
				setTimeout(() => resolve({ value: { payload: { type: "timeout" } }, done: false }), 20),
			),
		])
	).value.payload
}

async function nextPayloadOfType(stream: AsyncIterator<any>, type: string): Promise<any> {
	for (let index = 0; index < 3; index += 1) {
		const payload = await nextPayload(stream)
		if (payload.type === type || payload.type === "timeout") return payload
	}
	return { type: "timeout" }
}

describe("ACP desktop SDK provider error events", () => {
	test("normalizes scheduled and resumed provider retry statuses", async () => {
		const transport = new ErrorEventTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		await client.session.list()

		for (const phase of ["scheduled", "resumed"]) {
			transport.emitOriginalEvent("turn/provider_retry_status", {
				kind: "turn_provider_retry_status",
				session_id: "s1",
				turn_id: "t1",
				attempt: 2,
				backoff_ms: phase === "scheduled" ? 1000 : 0,
				provider: "openai",
				model: "test-model",
				phase,
				message: phase === "scheduled" ? "Retrying in 1.0s" : "Retrying now",
			})

			expect(await nextPayloadOfType(stream, "turn.provider_retry_status")).toEqual({
				type: "turn.provider_retry_status",
				properties: {
					sessionID: "s1",
					turnID: "t1",
					attempt: 2,
					backoffMs: phase === "scheduled" ? 1000 : 0,
					provider: "openai",
					model: "test-model",
					phase,
					message: phase === "scheduled" ? "Retrying in 1.0s" : "Retrying now",
				},
			})
		}
	})

	test("maps transient turn failures to session errors without assistant messages", async () => {
		const transport = new ErrorEventTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		await client.session.list()

		transport.emitOriginalEvent("turn/failed", {
			kind: "turn_failed",
			session_id: "s1",
			turn: { turn_id: "t1", status: "failed" },
			error: { code: "PROVIDER_SERVER_ERROR", message: "Internal server error" },
		})

		expect(await nextPayloadOfType(stream, "session.error")).toEqual({
			type: "session.error",
			properties: {
				sessionID: "s1",
				error: {
					name: "PROVIDER_SERVER_ERROR",
					data: { message: "Internal server error" },
				},
			},
		})
		expect(await nextPayload(stream)).toEqual({ type: "timeout" })
	})

	test("leaves error-less legacy and shell turn failures on their existing display paths", async () => {
		const transport = new ErrorEventTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		await client.session.list()

		transport.emitOriginalEvent("turn/failed", {
			kind: "turn_failed",
			session_id: "s1",
			turn: { turn_id: "t1", status: "failed" },
		})

		expect((await nextPayload(stream)).type).toBe("session.updated")
		expect(await nextPayload(stream)).toEqual({ type: "timeout" })
	})
})
