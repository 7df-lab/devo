import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly requests: string[] = []
	readonly listeners = new Set<(event: DevoNativeTransportEvent) => void>()
	private rejectFirstInitialize!: (error: Error) => void
	readonly firstInitialize = new Promise<unknown>((_resolve, reject) => {
		this.rejectFirstInitialize = reject
	})

	async request(method: string): Promise<unknown> {
		this.requests.push(method)
		if (method === "initialize") {
			if (this.requests.filter((request) => request === "initialize").length === 1) {
				return this.firstInitialize
			}
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		}
		if (method === "session/list") return { data: [], nextCursor: null }
		if (method === "subscription/create") return { subscriptionId: "folder", snapshots: [], cursors: [] }
		throw new Error(`unexpected request ${method}`)
	}

	failFirstInitialize(error: Error): void {
		this.rejectFirstInitialize(error)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.add(listener)
		return () => { this.listeners.delete(listener) }
	}

	connected(): boolean {
		return true
	}
}

describe("NativeClient initialize retry", () => {
	test("shares pending initialize, retries after rejection without a closed event, then caches success", async () => {
		const transport = new FakeTransport()
		const firstClient = createDevoClient({ directory: "/repo", transport })
		const secondClient = createDevoClient({ directory: "/repo", transport })
		try {
			const firstList = firstClient.session.list()
			const firstSubscribe = secondClient.event.subscribe()
			// Let both clients reach the same still-pending initialize request.
			await new Promise<void>((resolve) => setTimeout(resolve, 0))
			expect(transport.requests).toEqual(["initialize"])

			const transientError = new Error("temporary initialize failure")
			transport.failFirstInitialize(transientError)
			expect(await Promise.allSettled([firstList, firstSubscribe])).toEqual([
				{ status: "rejected", reason: transientError },
				{ status: "rejected", reason: transientError },
			])
			expect(transport.requests).toEqual(["initialize"])
			expect(transport.listeners.size).toBe(2) // No transport closed event occurred.

			expect(await secondClient.event.subscribe()).toEqual({ stream: expect.anything() })
			expect(await firstClient.session.list()).toEqual({ data: [] })
			expect(transport.requests).toEqual([
				"initialize", "initialize", "subscription/create", "session/list",
			])
			await firstClient.event.subscribe()
			await secondClient.session.list()
			expect(transport.requests.filter((method) => method === "initialize")).toHaveLength(2)
		} finally {
			firstClient.dispose()
			secondClient.dispose()
		}
	})
})
