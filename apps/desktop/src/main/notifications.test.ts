import { describe, expect, mock, test } from "bun:test"

class MockBrowserWindow {
	static windows: MockBrowserWindow[] = []
	static getAllWindows(): MockBrowserWindow[] {
		return MockBrowserWindow.windows
	}

	readonly calls: string[] = []
	readonly navigation: Array<{ sessionId: string; visible: boolean }> = []
	readonly webContents = {
		send: (channel: string, payload: { sessionId: string }) => {
			this.calls.push("send")
			if (channel !== "notification:navigate") throw new Error(`Unexpected channel: ${channel}`)
			this.navigation.push({ sessionId: payload.sessionId, visible: this.visible })
		},
	}

	constructor(
		private visible: boolean,
		private minimized: boolean,
		private restoreMakesVisible = true,
	) {}

	isDestroyed(): boolean {
		return false
	}
	isFocused(): boolean {
		return false
	}
	isVisible(): boolean {
		return this.visible
	}
	isMinimized(): boolean {
		return this.minimized
	}
	restore(): void {
		this.calls.push("restore")
		this.minimized = false
		if (this.restoreMakesVisible) this.visible = true
	}
	show(): void {
		this.calls.push("show")
		this.visible = true
	}
	focus(): void {
		this.calls.push("focus")
	}
}

class MockNotification {
	static latest: MockNotification | undefined
	static getLatest(): MockNotification | undefined {
		return MockNotification.latest
	}
	static isSupported(): boolean {
		return true
	}

	private readonly listeners = new Map<string, () => void>()
	constructor(_options: { title: string; body: string }) {
		MockNotification.latest = this
	}
	on(event: string, listener: () => void): void {
		this.listeners.set(event, listener)
	}
	show(): void {}
	close(): void {}
	click(): void {
		this.listeners.get("click")?.()
	}
}

mock.module("electron", () => ({
	BrowserWindow: MockBrowserWindow,
	Notification: MockNotification,
	app: {},
}))
mock.module("./settings-store", () => ({
	getNotificationSettings: () => ({ completionMode: "unfocused" }),
}))

describe("notification click navigation", () => {
	test.each([
		{
			name: "hidden",
			visible: false,
			minimized: false,
			restoreMakesVisible: true,
			calls: ["show", "focus", "send"],
		},
		{
			name: "minimized",
			visible: false,
			minimized: true,
			restoreMakesVisible: true,
			calls: ["restore", "focus", "send"],
		},
		{
			name: "visible",
			visible: true,
			minimized: false,
			restoreMakesVisible: true,
			calls: ["focus", "send"],
		},
		{
			name: "still hidden after restore",
			visible: false,
			minimized: true,
			restoreMakesVisible: false,
			calls: ["restore", "show", "focus", "send"],
		},
	])(
		"opens a $name window before sending navigation",
		async ({ visible, minimized, restoreMakesVisible, calls }) => {
			const window = new MockBrowserWindow(visible, minimized, restoreMakesVisible)
			MockBrowserWindow.windows = [window]
			MockNotification.latest = undefined
			const { showNotification } = await import("./notifications")
			const sessionId = `session-${minimized}-${visible}-${restoreMakesVisible}`

			showNotification({ type: "completed", sessionId, title: "Finished", body: "Task completed" })
			const notification = MockNotification.getLatest()
			expect(notification).toBeDefined()
			notification?.click()

			expect(window.calls).toEqual([...calls])
			expect(window.navigation).toEqual([{ sessionId, visible: true }])
		},
	)
})
