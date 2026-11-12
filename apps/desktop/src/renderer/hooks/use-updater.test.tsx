import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { UpdateState } from "../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
mock.module("../services/backend", () => ({ isElectron: true }))
const { useUpdater } = await import("./use-updater")
const { createRoot } = await import("react-dom/client")
let root: Root
let result: ReturnType<typeof useUpdater>
let snapshots: Array<{
	resolve: (state: UpdateState) => void
	reject: (error: Error) => void
}>
let listeners: Set<(state: UpdateState) => void>
let calls: string[]
function Probe() {
	result = useUpdater()
	return <span>{result.status}</span>
}
beforeEach(() => {
	snapshots = []
	listeners = new Set()
	calls = []
	Object.assign(window, {
		devo: {
			getUpdateState: () => {
				calls.push("snapshot")
				return new Promise((resolve, reject) => snapshots.push({ resolve, reject }))
			},
			onUpdateStateChanged: (listener: (state: UpdateState) => void) => {
				calls.push("subscribe")
				listeners.add(listener)
				return () => {
					listeners.delete(listener)
				}
			},
		},
	})
	root = createRoot(document.createElement("div"))
})
afterEach(async () => {
	await act(async () => root.unmount())
})
const ready: UpdateState = {
	status: "ready",
	version: "1.0.0",
	canAutoInstall: true,
}
const idle: UpdateState = { status: "idle", canAutoInstall: false }
test("initial snapshot cannot overwrite a newer pushed ready state", async () => {
	await act(async () => root.render(<Probe />))
	await act(async () => {
		for (const listener of listeners) listener(ready)
	})
	await act(async () => snapshots[0].resolve(idle))
	expect({
		status: result.status,
		version: result.version,
		canAutoInstall: result.canAutoInstall,
	}).toEqual({ status: "ready", version: "1.0.0", canAutoInstall: true })
})
test("subscribe before requesting the initial snapshot", async () => {
	await act(async () => root.render(<Probe />))
	expect(calls).toEqual(["subscribe", "snapshot"])
})
test("loads initial state when no live update has arrived", async () => {
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[0].resolve(idle))
	expect(result.canAutoInstall).toBe(false)
})
test("discard snapshot from disposed StrictMode effect", async () => {
	await act(async () =>
		root.render(
			<StrictMode>
				<Probe />
			</StrictMode>,
		),
	)
	expect(snapshots.length).toBe(2)
	expect(listeners.size).toBe(1)
	await act(async () => snapshots[1].resolve(ready))
	await act(async () => snapshots[0].resolve(idle))
	expect(result.status).toBe("ready")
})
test("snapshot failure does not prevent later live updates", async () => {
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[0].reject(new Error("unavailable")))
	await act(async () => {
		for (const listener of listeners) listener(ready)
	})
	expect(result.status).toBe("ready")
})

test("continues applying progress and terminal events after hydration", async () => {
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[0].resolve(idle))
	const downloading: UpdateState = {
		status: "downloading",
		version: "1.0.0",
		canAutoInstall: true,
		progress: {
			percent: 50,
			bytesPerSecond: 100,
			transferred: 100,
			total: 200,
		},
	}
	await act(async () => {
		for (const listener of listeners) listener(downloading)
	})
	expect(result.progress).toEqual(downloading.progress)
	await act(async () => {
		for (const listener of listeners) listener(ready)
	})
	expect(result.status).toBe("ready")
	expect(result.progress).toBeUndefined()
})

test("cleanup unsubscribes and ignores a queued callback", async () => {
	await act(async () =>
		root.render(
			<StrictMode>
				<Probe />
			</StrictMode>,
		),
	)
	const [callback] = listeners
	await act(async () => root.render(null))
	expect(listeners.size).toBe(0)
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[2].resolve(ready))
	await act(async () => callback(idle))
	expect(result.status).toBe("ready")
})
