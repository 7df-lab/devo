import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { AppSettings } from "../../preload/api"
import { DEFAULT_APP_SETTINGS } from "../../shared/app-settings"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
Object.assign(window, { devo: {} })
const { useSettings } = await import("./use-settings")
const { createRoot } = await import("react-dom/client")
let root: Root
let result: ReturnType<typeof useSettings>
type Pending = { resolve: (s: AppSettings) => void; reject: (e: Error) => void }
let snapshots: Pending[]
let updates: Pending[]
let listeners: Set<(s: AppSettings) => void>
let calls: string[]
function Probe() { result = useSettings(); return <span>{String(result.settings.opaqueWindows)}</span> }
const initial = DEFAULT_APP_SETTINGS
const newer = { ...initial, opaqueWindows: true }
const newest = { ...newer, notifications: { ...initial.notifications, errors: false } }
const push = async (s: AppSettings) => act(async () => { for (const cb of listeners) cb(s) })
beforeEach(() => {
	snapshots = []; updates = []; listeners = new Set(); calls = []
	Object.assign(window, { devo: {
		getSettings: () => { calls.push("snapshot"); return new Promise((resolve, reject) => snapshots.push({ resolve, reject })) },
		updateSettings: () => new Promise((resolve, reject) => updates.push({ resolve, reject })),
		onSettingsChanged: (cb: (s: AppSettings) => void) => { calls.push("subscribe"); listeners.add(cb); return () => listeners.delete(cb) },
	} })
	root = createRoot(document.createElement("div"))
})
afterEach(async () => { await act(async () => root.unmount()) })
test("subscribes before snapshot and loads initial settings", async () => {
	await act(async () => root.render(<Probe />))
	expect(calls).toEqual(["subscribe", "snapshot"])
	await act(async () => snapshots[0].resolve(newer))
	expect({ settings: result.settings, loading: result.loading }).toEqual({ settings: newer, loading: false })
})
test("live push supersedes delayed initial snapshot", async () => {
	await act(async () => root.render(<Probe />)); await push(newer)
	await act(async () => snapshots[0].resolve(initial))
	expect(result.settings).toEqual(newer)
})
test("disposed StrictMode snapshot cannot replace active effect state", async () => {
	await act(async () => root.render(<StrictMode><Probe /></StrictMode>))
	expect(listeners.size).toBe(1)
	await act(async () => snapshots[1].resolve(newer))
	await act(async () => snapshots[0].resolve(initial))
	expect(result.settings).toEqual(newer)
})
test("older mutation response cannot overwrite newer response", async () => {
	await act(async () => root.render(<Probe />))
	const first = result.updateSettings({ opaqueWindows: true })
	const second = result.updateSettings({ notifications: { errors: false } })
	await act(async () => { updates[1].resolve(newest); await second })
	await act(async () => { updates[0].resolve(newer); await first })
	expect(result.settings).toEqual(newest)
})
test("live push supersedes in-flight mutation response", async () => {
	await act(async () => root.render(<Probe />))
	const pending = result.updateSettings({ opaqueWindows: true })
	await push(newest)
	await act(async () => { updates[0].resolve(newer); await pending })
	expect(result.settings).toEqual(newest)
})
test("failed mutation does not roll back newer pushed settings", async () => {
	await act(async () => root.render(<Probe />))
	const pending = result.updateSettings({ opaqueWindows: true })
	await push(newest)
	await act(async () => { updates[0].reject(new Error("rejected")); await pending })
	expect(result.settings).toEqual(newest)
})
test("initial snapshot cannot regress a completed mutation", async () => {
	await act(async () => root.render(<Probe />))
	const pending = result.updateSettings({ opaqueWindows: true })
	await act(async () => { updates[0].resolve(newer); await pending })
	await act(async () => snapshots[0].resolve(initial))
	expect(result.settings).toEqual(newer)
})
test("failed mutation before initial load still allows snapshot", async () => {
	await act(async () => root.render(<Probe />))
	const pending = result.updateSettings({ opaqueWindows: false })
	await act(async () => { updates[0].reject(new Error("rejected")); await pending })
	await act(async () => snapshots[0].resolve(newer))
	expect(result.settings).toEqual(newer)
})
test("snapshot failure ends loading without disabling future pushes", async () => {
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[0].reject(new Error("unavailable")))
	expect(result.loading).toBe(false)
	await push(newer)
	expect(result.settings).toEqual(newer)
})
test("unmounted mutation cannot affect a newly mounted hook", async () => {
	await act(async () => root.render(<Probe />))
	const pending = result.updateSettings({ opaqueWindows: true })
	await act(async () => root.render(null))
	await act(async () => root.render(<Probe />))
	await act(async () => snapshots[1].resolve(newest))
	await act(async () => { updates[0].resolve(newer); await pending })
	expect(result.settings).toEqual(newest)
})
