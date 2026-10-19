import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly listeners = new Set<(event: DevoNativeTransportEvent) => void>()
	unsubscribeCalls = 0

	async request(method: string): Promise<unknown> {
		if (method === "initialize") {
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		}
		if (method === "session/list") {
			return { data: [], nextCursor: null }
		}
		throw new Error(`unexpected request ${method}`)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.add(listener)
		return () => {
			this.unsubscribeCalls += 1
			this.listeners.delete(listener)
		}
	}

	connected(): boolean {
		return true
	}
}

describe("NativeClient disposal", () => {
	test("unsubscribes once and closes an active event stream", async () => {
		const transport = new FakeTransport()
		const client = createDevoClient({ transport })
		const { stream } = await client.event.subscribe()
		const pendingNext = stream[Symbol.asyncIterator]().next()

		expect(transport.listeners.size).toBe(1)
		client.dispose()

		expect(transport.listeners.size).toBe(0)
		expect(transport.unsubscribeCalls).toBe(1)
		expect(await pendingNext).toEqual({ value: undefined, done: true })

		client.dispose()
		expect(transport.unsubscribeCalls).toBe(1)
	})
})
