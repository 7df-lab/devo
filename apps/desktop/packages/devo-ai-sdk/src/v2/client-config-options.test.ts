import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoAcpTransport, type DevoAcpTransportEvent } from "./client"
import type { AcpSessionConfigOption, AcpSessionNotification } from "./generated"

class FakeTransport implements DevoAcpTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	private listeners: Array<(event: DevoAcpTransportEvent) => void> = []

	constructor(
		private readonly handler: (
			method: string,
			params: unknown,
			directory?: string,
		) => unknown,
	) {}

	async request(method: string, params?: unknown, directory?: string): Promise<unknown> {
		this.requests.push({ method, params, directory })
		return this.handler(method, params, directory)
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

const configOptions = [
	{
		type: "select",
		id: "model",
		name: "Model",
		category: "model",
		currentValue: "test-openai",
		options: [
			{ value: "test-openai", name: "Test OpenAI", description: "OpenAI: test-model" },
			{ value: "alt-openai", name: "Alt OpenAI", description: "OpenAI: alt-model" },
		],
	},
] satisfies AcpSessionConfigOption[]

describe("ACP desktop SDK config option cache", () => {
	test("loads cold-start config options from model/config when no session cache exists", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "model/config") {
				expect(params).toEqual({ cwd: "/repo" })
				return { configOptions }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const providers = await client.config.providers()
		const config = await client.config.get()

		expect(providers.data.default).toEqual({ session: "test-openai" })
		expect(config.data).toEqual({ model: "session/test-openai" })
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"model/config",
		])
	})

	test("keeps session model options when a live config update is empty", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/new") return { sessionId: "s1", configOptions }
			if (method === "model/config") throw new Error("model/config should not be called")
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.create()
		const before = await client.config.providers()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "config_option_update",
				configOptions: [],
			},
		} satisfies AcpSessionNotification)
		const after = await client.config.providers()

		expect(after.data).toEqual(before.data)
	})
})
