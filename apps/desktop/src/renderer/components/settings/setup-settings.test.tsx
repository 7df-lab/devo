import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { DevoCheckResult } from "../../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
Object.assign(window, { devo: {} })
const { createRoot } = await import("react-dom/client")
const { SetupSettings } = await import("./setup-settings")
type Pending = { resolve: (value: DevoCheckResult) => void; reject: (error: unknown) => void }
let requests: Pending[]
let root: Root
let mount: HTMLDivElement
const good = { installed: true, compatible: true, compatibility: "ok", message: null, version: "0.2.20", path: "/runtime/devo" } satisfies DevoCheckResult
const missing = { installed: false, compatible: false, compatibility: "unknown", version: null, path: null, message: "Runtime missing" } satisfies DevoCheckResult
const blocked = { ...good, compatible: false, compatibility: "blocked", message: "Blocked runtime" } satisfies DevoCheckResult
function check() { return [...mount.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Check")! }
async function render(strict = false) {
	await act(async () => root.render(strict ? <StrictMode><SetupSettings /></StrictMode> : <SetupSettings />))
}
async function settle(index: number, value: DevoCheckResult) { await act(async () => requests[index].resolve(value)) }
function snapshot() { return { version: mount.textContent?.includes("v0.2.20"), disabled: check().disabled,
	alert: mount.querySelector('[role="alert"]')?.textContent ?? null } }
beforeEach(() => {
	requests = []
	Object.assign(window, { devo: { onboarding: { checkDevo: () => new Promise((resolve, reject) => requests.push({ resolve, reject })) } } })
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
})
afterEach(async () => { await act(async () => root.unmount()); mount.remove() })
test("successful runtime check displays its version and permits recheck", async () => {
	await render(); expect(check().disabled).toBe(true); await settle(0, good)
	expect(snapshot()).toEqual({ version: true, disabled: false, alert: null })
})
test("failed recheck clears the previous success and reports the error", async () => {
	await render(); await settle(0, good); await act(async () => check().click())
	await act(async () => requests[1].reject(new Error("Runtime IPC failed")))
	expect(snapshot()).toEqual({ version: false, disabled: false, alert: "Runtime IPC failed" })
	expect(mount.textContent?.includes("Not found")).toBe(false)
})
test("retry clears the failure and allows a new successful result", async () => {
	await render(); await act(async () => requests[0].reject(new Error("offline")))
	await act(async () => check().click())
	expect(snapshot()).toEqual({ version: false, disabled: true, alert: null })
	await settle(1, good); expect(snapshot()).toEqual({ version: true, disabled: false, alert: null })
})
test("non Error rejection reports a useful fallback instead of Not found", async () => {
	await render(); await act(async () => requests[0].reject("transport unavailable"))
	expect(snapshot()).toEqual({ version: false, disabled: false, alert: "Failed to check Desktop runtime" })
})
test("StrictMode disposed success cannot replace latest missing runtime", async () => {
	await render(true); expect(requests.length).toBe(2)
	await settle(1, missing); await settle(0, good)
	expect(snapshot()).toEqual({ version: false, disabled: false, alert: null })
	expect(mount.textContent?.includes("Runtime missing")).toBe(true)
})
test("disposed failure cannot clear the current check pending state", async () => {
	await render(true); await act(async () => requests[0].reject(new Error("obsolete")))
	expect(snapshot()).toEqual({ version: false, disabled: true, alert: null })
	await settle(1, blocked)
	expect(mount.textContent?.includes("Blocked runtime")).toBe(true)
})
test("disposed success cannot finish the current pending check", async () => {
	await render(true); await settle(0, good)
	expect(snapshot()).toEqual({ version: false, disabled: true, alert: null })
	await settle(1, missing)
})
test("old instance responses cannot change a remounted section", async () => {
	await render(); await act(async () => root.render(null)); await render()
	await settle(1, missing); await settle(0, good)
	expect(snapshot()).toEqual({ version: false, disabled: false, alert: null })
	expect(mount.textContent?.includes("Runtime missing")).toBe(true)
})

test("empty Error messages still show a failed check rather than a missing runtime", async () => {
	await render(); await act(async () => requests[0].reject(new Error("  ")))
	expect(snapshot()).toEqual({ version: false, disabled: false, alert: "Failed to check Desktop runtime" })
})
