import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { DevoCheckResult } from "../../../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
const { createRoot } = await import("react-dom/client")
const { EnvironmentCheckStep } = await import("./environment-check-step")
type Pending = { resolve: (value: DevoCheckResult) => void; reject: (error: Error) => void }
let requests: Pending[]
let completed: (string | null)[]
let root: Root
let mount: HTMLDivElement
const good = { installed: true, compatible: true, compatibility: "ok", message: null, version: "0.2.20", path: "/runtime/devo" } satisfies DevoCheckResult
const missing = { installed: false, compatible: false, compatibility: "unknown", version: null, path: null, message: "Runtime missing" } satisfies DevoCheckResult
const blocked = { ...good, compatible: false, compatibility: "blocked", message: "Blocked runtime" } satisfies DevoCheckResult
function button(label: string) { return [...mount.querySelectorAll("button")].find((b) => b.textContent?.trim() === label) }
async function render(strict = false) {
	const tree = <EnvironmentCheckStep onComplete={(version) => completed.push(version)} />
	await act(async () => root.render(strict ? <StrictMode>{tree}</StrictMode> : tree))
}
async function settle(index: number, value: DevoCheckResult) { await act(async () => requests[index].resolve(value)) }
async function finishAnimation() { await act(async () => { await new Promise((resolve) => setTimeout(resolve, 320)) }) }
beforeEach(() => {
	requests = []; completed = []
	Object.assign(window, { devo: { onboarding: { checkDevo: () => new Promise((resolve, reject) => requests.push({ resolve, reject })) } } })
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
})
afterEach(async () => { await act(async () => root.unmount()); mount.remove() })
test("compatible runtime enables Continue with the checked version", async () => {
	await render(); await settle(0, good); await finishAnimation()
	await act(async () => button("Continue")?.click())
	expect(completed).toEqual(["0.2.20"])
})
test("older success cannot replace a newer failed check", async () => {
	await render(); await act(async () => button("Re-check")?.click())
	await settle(1, missing); await settle(0, good); await finishAnimation()
	expect({ continue: !!button("Continue"), missing: mount.textContent?.includes("Runtime missing") }).toEqual({ continue: false, missing: true })
})
test("success delayed for animation cannot enable Continue after a failed retry", async () => {
	await render(); await settle(0, good)
	await act(async () => button("Re-check")?.click()); await settle(1, missing); await finishAnimation()
	expect(!!button("Continue")).toBe(false)
})
test("older rejection cannot overwrite a successful retry", async () => {
	await render(); await act(async () => button("Re-check")?.click()); await settle(1, good); await finishAnimation()
	await act(async () => requests[0].reject(new Error("obsolete failure")))
	expect({ continue: !!button("Continue"), stale: mount.textContent?.includes("obsolete failure") }).toEqual({ continue: true, stale: false })
})
test("blocked runtime cannot be replaced by a previous compatible version", async () => {
	await render(); await settle(0, good); await act(async () => button("Re-check")?.click()); await settle(1, blocked); await finishAnimation()
	expect({ continue: !!button("Continue"), blocked: mount.textContent?.includes("Version blocked") }).toEqual({ continue: false, blocked: true })
})
test("StrictMode starts a fresh check and rejects the disposed effect response", async () => {
	await render(true); expect(requests.length).toBe(2)
	await settle(1, missing); await settle(0, good); await finishAnimation()
	expect({ continue: !!button("Continue"), missing: mount.textContent?.includes("Runtime missing") }).toEqual({ continue: false, missing: true })
})
test("unmount and remount cannot inherit the previous check", async () => {
	await render(); await act(async () => root.render(null)); await render()
	await settle(1, missing); await settle(0, good); await finishAnimation()
	expect({ continue: !!button("Continue"), completed }).toEqual({ continue: false, completed: [] })
})
