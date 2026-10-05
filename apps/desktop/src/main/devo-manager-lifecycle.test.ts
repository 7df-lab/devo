import { afterEach, describe, expect, mock, test } from "bun:test"

type Deferred<T> = {
	promise: Promise<T>
	resolve: (value: T) => void
	reject: (error: Error) => void
}

function deferred<T>(): Deferred<T> {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}

const envGates: Deferred<void>[] = []
const initializeGates: Deferred<unknown>[] = []
const clients: FakeClient[] = []
let watcherStarts = 0
let watcherStops = 0

class FakeClient {
	readonly pidValue = 100 + clients.length
	readonly listeners = new Set<(event: { type: "closed"; error: string }) => void>()
	starts = 0
	stops = 0
	initializeCalls = 0
	constructor(_options: unknown) {
		clients.push(this)
	}
	start(): void {
		this.starts++
	}
	stop(): void {
		this.stops++
	}
	connected(): boolean {
		return this.starts > 0 && this.stops === 0
	}
	pid(): number {
		return this.pidValue
	}
	subscribe(listener: (event: { type: "closed"; error: string }) => void): () => void {
		this.listeners.add(listener)
		return () => this.listeners.delete(listener)
	}
	request(method: string): Promise<unknown> {
		if (method !== "initialize") throw new Error(`unexpected request: ${method}`)
		this.initializeCalls++
		return initializeGates.shift()?.promise ?? Promise.resolve({})
	}
	emitClosed(): void {
		for (const listener of this.listeners) listener({ type: "closed", error: "old process closed" })
	}
}

mock.module("electron", () => ({
	app: { getAppPath: () => "/fake/app", isPackaged: false },
}))
mock.module("./shell-env", () => ({ waitForEnv: () => envGates.shift()?.promise ?? Promise.resolve() }))
mock.module("./native-stdio-client", () => ({ StdioNativeClient: FakeClient }))
mock.module("./devo-program", () => ({ resolveDevoProgram: () => "/fake/devo" }))
mock.module("./settings-store", () => ({ getSettings: () => ({ servers: {} }) }))
mock.module("./native-traffic-log", () => ({
	DEVO_HOME_ENV: "DEVO_HOME",
	PROTOCOL_TRACE_ENV: "DEVO_PROTOCOL_TRACE",
	PROTOCOL_TRACE_FILE_ENV: "DEVO_PROTOCOL_TRACE_FILE",
	createNativeTrafficLoggerFromEnv: () => ({ getState: () => ({}), record: () => {} }),
}))
mock.module("./notification-watcher", () => ({
	startNotificationWatcher: () => { watcherStarts++ },
	stopNotificationWatcher: () => { watcherStops++ },
}))
mock.module("./logger", () => ({
	createLogger: () => ({ info: () => {}, warn: () => {}, debug: () => {}, error: () => {} }),
}))

const manager = await import("./devo-manager")

afterEach(() => {
	manager.stopServer()
	envGates.length = 0
	initializeGates.length = 0
	clients.length = 0
	watcherStarts = 0
	watcherStops = 0
})

describe("desktop server lifecycle", () => {
	test("stop during shell environment wait prevents spawning or publishing", async () => {
		const env = deferred<void>()
		envGates.push(env)
		const ready = manager.ensureServer()
		const onReady = mock(() => {})
		const unsubscribe = manager.onServerReady(onReady)
		expect(manager.stopServer()).toBe(false)
		env.resolve()
		await expect(ready).rejects.toThrow(/cancelled/)
		expect(clients).toHaveLength(0)
		expect(watcherStarts).toBe(0)
		expect(onReady).not.toHaveBeenCalled()
		expect(manager.getServerUrl()).toBeNull()
		unsubscribe()
	})

	test("stop during initialize stops child and prevents stale ready publication", async () => {
		const initialize = deferred<unknown>()
		initializeGates.push(initialize)
		const onReady = mock(() => {})
		const unsubscribe = manager.onServerReady(onReady)
		const ready = manager.ensureServer()
		await Promise.resolve() // waitForEnv completed; initialize is pending
		const child = clients[0]!
		expect(child.starts).toBe(1)
		expect(manager.stopServer()).toBe(true)
		expect(child.stops).toBe(1)
		initialize.resolve({})
		await expect(ready).rejects.toThrow(/cancelled/)
		expect(child.stops).toBe(1)
		expect(watcherStarts).toBe(0)
		expect(onReady).not.toHaveBeenCalled()
		expect(manager.getServerUrl()).toBeNull()
		unsubscribe()
	})

	test("restart runs while old startup is pending and ignores old completion and close", async () => {
		const oldEnv = deferred<void>()
		const newInitialize = deferred<unknown>()
		envGates.push(oldEnv)
		initializeGates.push(newInitialize)
		const oldReady = manager.ensureServer()
		const newReady = manager.restartServer()
		await Promise.resolve() // restart enters initialize
		expect(clients).toHaveLength(1)
		const concurrentReady = manager.ensureServer()
		oldEnv.resolve()
		await expect(oldReady).rejects.toThrow(/cancelled/)
		const alsoConcurrent = manager.ensureServer()
		await Promise.resolve()
		expect(clients[0]?.initializeCalls).toBe(1)
		newInitialize.resolve({})
		const ready = await newReady
		expect(await concurrentReady).toBe(ready)
		expect(await alsoConcurrent).toBe(ready)
		expect(manager.getServerUrl()).toBe("stdio://local")
		expect(clients).toHaveLength(1)
		expect(watcherStarts).toBe(1)
	})

	test("old child completion and close cannot overwrite replacement", async () => {
		const oldInitialize = deferred<unknown>()
		initializeGates.push(oldInitialize)
		const oldReady = manager.ensureServer()
		await Promise.resolve()
		const oldChild = clients[0]!
		const ready = await manager.restartServer()
		expect(oldChild.stops).toBe(1)
		oldInitialize.resolve({})
		await expect(oldReady).rejects.toThrow(/cancelled/)
		oldChild.emitClosed()
		expect(manager.getServerUrl()).toBe(ready.url)
		expect(await manager.ensureServer()).toBe(ready)
		expect(clients).toHaveLength(2)
		expect(clients[1]?.stops).toBe(0)
		expect(watcherStarts).toBe(1)
	})
})
