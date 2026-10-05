import { afterEach, describe, expect, mock, test } from "bun:test"
import type { DevoNativeTransport, DevoNativeTransportEvent } from "@devo-ai/sdk/v2/client"

mock.module("./notifications", () => ({
	setPermissionResponder: () => {},
	showNotification: () => {},
	updateBadgeCount: () => {},
}))

mock.module("./devo-manager", () => ({
	recycleServerForProtocolMismatch: async () => false,
}))

class FakeTransport implements DevoNativeTransport {
	readonly listeners = new Set<(event: DevoNativeTransportEvent) => void>()
	subscribeCalls = 0
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
		this.subscribeCalls += 1
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

async function tick(): Promise<void> {
	await new Promise((resolve) => setTimeout(resolve, 0))
}

afterEach(async () => {
	const watcher = await import("./notification-watcher")
	watcher.stopNotificationWatcher()
	await tick()
})

describe("notification watcher client cleanup", () => {
	test("unsubscribes the replaced client and closes the stopped stream", async () => {
		const watcher = await import("./notification-watcher")
		const transport = new FakeTransport()

		watcher.startNotificationWatcher(transport)
		await tick()
		expect(transport.subscribeCalls).toBe(1)
		expect(transport.listeners.size).toBe(1)

		watcher.startNotificationWatcher(transport)
		await tick()
		expect(transport.subscribeCalls).toBe(2)
		expect(transport.unsubscribeCalls).toBe(1)
		expect(transport.listeners.size).toBe(1)

		watcher.stopNotificationWatcher()
		await tick()
		expect(transport.unsubscribeCalls).toBe(2)
		expect(transport.listeners.size).toBe(0)
	})
})
