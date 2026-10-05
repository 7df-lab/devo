import { EventEmitter } from "node:events"
import path from "node:path"
import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"

const createdImagePaths: string[] = []
const resizedImages: Array<{ height: number; width: number }> = []
const templateImageFlags: boolean[] = []
const trayInstances: FakeElectronTray[] = []
const serverReadyListeners: Array<() => void> = []
const sessionListParams: unknown[] = []
const nativeEventEmitter = new EventEmitter()
let nativeTransportSubscribeCount = 0
let nativeTransportUnsubscribeCount = 0

let serverUrl: string | null = null
let discoveredSessions: unknown[] = []
let sessionListGate: Promise<void> | null = null

const fakeNativeTransport = {
	request: async (method: string, params?: unknown) => {
		if (method === "initialize") {
			return {
				protocolVersion: 1,
				agentCapabilities: {},
				authMethods: [],
			}
		}
		if (method === "session/list") {
			sessionListParams.push(params)
			if (sessionListGate) await sessionListGate
			return { data: discoveredSessions, nextCursor: null }
		}
		throw new Error(`unexpected request ${method}`)
	},
	respond: async () => {},
	subscribe: (listener: (...args: unknown[]) => void) => {
		nativeTransportSubscribeCount += 1
		nativeEventEmitter.on("native-event", listener)
		let subscribed = true
		return () => {
			if (!subscribed) return
			subscribed = false
			nativeEventEmitter.off("native-event", listener)
			nativeTransportUnsubscribeCount += 1
		}
	},
	connected: () => true,
}

class FakeNativeImage {
	constructor(readonly imagePath: string) {}

	isEmpty(): boolean {
		return false
	}

	resize(options: { height: number; width: number }): FakeNativeImage {
		resizedImages.push(options)
		return this
	}

	setTemplateImage(flag: boolean): void {
		templateImageFlags.push(flag)
	}
}

class FakeElectronTray {
	readonly icon: FakeNativeImage
	tooltip = ""
	title = ""
	contextMenu: unknown = null
	destroyed = false

	constructor(icon: FakeNativeImage) {
		this.icon = icon
		trayInstances.push(this)
	}

	setToolTip(tooltip: string): void {
		this.tooltip = tooltip
	}

	setContextMenu(contextMenu: unknown): void {
		this.contextMenu = contextMenu
	}

	setTitle(title: string): void {
		this.title = title
	}

	destroy(): void {
		this.destroyed = true
	}
}

mock.module("electron", () => ({
	app: { isPackaged: false },
	BrowserWindow: class {},
	Menu: { buildFromTemplate: (template: unknown) => template },
	Notification: class {},
	nativeImage: {
		createFromPath: (imagePath: string) => {
			createdImagePaths.push(imagePath)
			return new FakeNativeImage(imagePath)
		},
	},
	Tray: FakeElectronTray,
}))

mock.module("./devo-manager", () => ({
	getNativeTransport: () => fakeNativeTransport,
	getServerUrl: () => serverUrl,
	onServerReady: (listener: () => void) => {
		serverReadyListeners.push(listener)
		return () => {
			const index = serverReadyListeners.indexOf(listener)
			if (index >= 0) serverReadyListeners.splice(index, 1)
		}
	},
}))

mock.module("./notification-watcher", () => ({
	getPendingCount: () => 0,
	getSessionStates: () => new Map(),
	onStateChanged: () => () => {},
}))

class FakeTray {
	readonly events: string[] = []
	private readonly listeners = new Map<string, Array<() => void>>()

	on(event: string, listener: () => void): this {
		this.events.push(event)
		const listeners = this.listeners.get(event) ?? []
		listeners.push(listener)
		this.listeners.set(event, listeners)
		return this
	}

	emit(event: string): void {
		for (const listener of this.listeners.get(event) ?? []) {
			listener()
		}
	}
}

beforeEach(() => {
	createdImagePaths.length = 0
	resizedImages.length = 0
	templateImageFlags.length = 0
	trayInstances.length = 0
	serverReadyListeners.length = 0
	sessionListParams.length = 0
	nativeEventEmitter.removeAllListeners("native-event")
	nativeTransportSubscribeCount = 0
	nativeTransportUnsubscribeCount = 0
	serverUrl = null
	discoveredSessions = []
	sessionListGate = null
})

afterEach(async () => {
	const { destroyTray } = await import("./tray")
	destroyTray()
})

describe("installTrayIconInteractions", () => {
	test("opens the desktop window when the Windows tray icon is clicked", async () => {
		const { installTrayIconInteractions } = await import("./tray")
		const tray = new FakeTray()
		let showWindowCalls = 0

		installTrayIconInteractions(
			tray,
			{
				showWindow: () => {
					showWindowCalls += 1
				},
			},
			"win32",
		)

		expect(tray.events).toEqual(["click"])
		tray.emit("click")
		expect(showWindowCalls).toBe(1)
	})

	test("does not bind tray icon clicks off Windows", async () => {
		const { installTrayIconInteractions } = await import("./tray")
		const tray = new FakeTray()
		let showWindowCalls = 0

		installTrayIconInteractions(
			tray,
			{
				showWindow: () => {
					showWindowCalls += 1
				},
			},
			"darwin",
		)

		expect(tray.events).toEqual([])
		tray.emit("click")
		expect(showWindowCalls).toBe(0)
	})
})

describe("createTray", () => {
	const testOnMac = process.platform === "darwin" ? test : test.skip

	test("uses the desktop tray icon on Windows", async () => {
		const { createTrayIcon } = await import("./tray")

		createTrayIcon(path.join(process.cwd(), "resources"), "win32")

		expect(createdImagePaths.map((imagePath) => path.basename(imagePath))).toEqual(["iconTray.png"])
		expect(resizedImages).toEqual([{ height: 18, width: 18 }])
		expect(templateImageFlags).toEqual([])
	})

	testOnMac("uses the full-color tray icon on macOS", async () => {
		const { createTray } = await import("./tray")

		createTray(() => undefined)

		expect(createdImagePaths.map((imagePath) => path.basename(imagePath))).toEqual(["iconTray.png"])
		expect(resizedImages).toEqual([{ height: 18, width: 18 }])
		expect(templateImageFlags).toEqual([])
		expect(trayInstances).toHaveLength(1)
	})

	test("does not populate Recent when the tray is created before server readiness", async () => {
		const { createTray } = await import("./tray")

		createTray(() => undefined)
		await new Promise((resolve) => setTimeout(resolve, 0))

		expect(sessionListParams).toEqual([])
		expect(
			(trayInstances[0].contextMenu as Array<{ label?: string }>).map((item) => item.label),
		).not.toContain("Recent")
	})

	test("refreshes Recent immediately when the managed server becomes ready", async () => {
		const { createTray } = await import("./tray")
		discoveredSessions = [
			{
				id: "ready-session",
				version: 1,
				title: "Ready from server",
				titleState: "unset",
				cwd: "/repo",
				ephemeral: false,
				createdAt: "1970-01-01T00:00:01.000Z",
				lastActivityAt: "1970-01-01T00:00:02.000Z",
				status: "idle",
				flags: [],
				archived: false,
				activity: "idle",
				queuedCount: 0,
				model: { provider: "", model: "" },
				settings: { permissionProfile: "default" },
				preview: "",
				usage: {
					total: {
						totalTokens: 0,
						inputTokens: 0,
						outputTokens: 0,
						reasoningTokens: 0,
						cacheReadInputTokens: 0,
						cacheCreationInputTokens: 0,
						callCount: 0,
						meteredCallCount: 0,
						failedCallCount: 0,
						cancelledCallCount: 0,
					},
					byPurpose: [],
					updatedAt: "1970-01-01T00:00:02.000Z",
				},
			},
		]

		createTray(() => undefined)
		serverUrl = "stdio://local"
		serverReadyListeners[0]()
		await new Promise((resolve) => setTimeout(resolve, 0))

		expect(sessionListParams).toHaveLength(2)
		expect(
			(trayInstances[0].contextMenu as Array<{ label?: string }>).map((item) => item.label),
		).toContain("Ready from server")
	})

	test("disposes each transient discovery client after its refresh", async () => {
		serverUrl = "stdio://local"
		const { createTray } = await import("./tray")

		createTray(() => undefined)
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(nativeTransportSubscribeCount).toBe(1)
		expect(nativeTransportUnsubscribeCount).toBe(1)

		serverReadyListeners[0]?.()
		await new Promise((resolve) => setTimeout(resolve, 0))
		expect(nativeTransportSubscribeCount).toBe(2)
		expect(nativeTransportUnsubscribeCount).toBe(2)
	})

	test("does not accumulate Native listeners across scheduled discovery refreshes", async () => {
		serverUrl = "stdio://local"
		const { createTray, destroyTray } = await import("./tray")
		const originalSetInterval = globalThis.setInterval
		const originalClearInterval = globalThis.clearInterval
		const intervalCallbacks: Array<Parameters<typeof setInterval>[0]> = []
		globalThis.setInterval = ((callback: Parameters<typeof setInterval>[0]) => {
			intervalCallbacks.push(callback)
			return 1 as unknown as ReturnType<typeof setInterval>
		}) as typeof setInterval
		globalThis.clearInterval = (() => {}) as typeof clearInterval
		const warnings: Error[] = []
		const onWarning = (warning: Error) => {
			if (warning.name === "MaxListenersExceededWarning") warnings.push(warning)
		}
		process.on("warning", onWarning)
		const tick = () => new Promise((resolve) => setTimeout(resolve, 0))

		try {
			expect(nativeEventEmitter.getMaxListeners()).toBe(10)
			createTray(() => undefined)
			await tick()
			const refreshInterval = intervalCallbacks[0]
			expect(typeof refreshInterval).toBe("function")
			if (typeof refreshInterval !== "function") throw new Error("Tray refresh interval was not registered")

			for (let refresh = 0; refresh < 12; refresh++) {
				refreshInterval()
				await tick()
				expect(nativeEventEmitter.listenerCount("native-event")).toBe(0)
			}
			await tick()

			expect(nativeTransportSubscribeCount).toBe(13)
			expect(nativeTransportUnsubscribeCount).toBe(13)
			expect(nativeEventEmitter.listenerCount("native-event")).toBe(0)
			expect(warnings).toEqual([])
		} finally {
			destroyTray()
			process.off("warning", onWarning)
			globalThis.setInterval = originalSetInterval
			globalThis.clearInterval = originalClearInterval
		}
	})

	test("does not overlap server-ready tray discovery refreshes", async () => {
		const { createTray } = await import("./tray")
		let releaseSessionList: () => void = () => {}
		sessionListGate = new Promise((resolve) => {
			releaseSessionList = resolve
		})

		createTray(() => undefined)
		serverUrl = "stdio://local"
		serverReadyListeners[0]()
		serverReadyListeners[0]()
		await new Promise((resolve) => setTimeout(resolve, 0))

		expect(sessionListParams).toHaveLength(2)

		releaseSessionList()
		await new Promise((resolve) => setTimeout(resolve, 0))
	})

	test("unsubscribes the server-ready listener when the tray is destroyed", async () => {
		const { createTray, destroyTray } = await import("./tray")

		createTray(() => undefined)
		expect(serverReadyListeners).toHaveLength(1)

		destroyTray()

		expect(serverReadyListeners).toHaveLength(0)
	})
})
