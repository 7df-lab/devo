import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { Automation, AutomationRun } from "../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
Object.assign(window, { devo: {} })
const { useAutomationData } = await import("./use-automation-data")
const { automationsAtom, automationRunsAtom } = await import("../atoms/automations")
const { createRoot } = await import("react-dom/client")
type Pending<T> = { resolve: (value: T) => void; reject: (error: Error) => void }
let root: Root
let store: ReturnType<typeof createStore>
let result: ReturnType<typeof useAutomationData>
let automations: Pending<Automation[]>[]
let runs: Pending<AutomationRun[]>[]
let listeners: Set<() => void>
let calls: string[]
function Probe() { result = useAutomationData(); return <span>{String(result.loading)}</span> }
function render(strict = false) {
	const tree = <Provider store={store}><Probe /></Provider>
	return act(async () => root.render(strict ? <StrictMode>{tree}</StrictMode> : tree))
}
// Minimal data is sufficient: the gateway only inspects these fields for logging.
const oldAutomation = { id: "a", name: "old", status: "paused" } as Automation
const newAutomation = { ...oldAutomation, name: "new" }
const oldRun = { id: "r", automationId: "a", workspace: "/workspace", status: "running" } as AutomationRun
const newRun = { ...oldRun, status: "pending_review" as const }
function settle(index: number, fresh: boolean) {
	return act(async () => {
		automations[index].resolve([fresh ? newAutomation : oldAutomation])
		runs[index].resolve([fresh ? newRun : oldRun])
	})
}
function snapshot() {
	return { automations: store.get(automationsAtom), runs: store.get(automationRunsAtom),
		loading: result.loading, error: result.error }
}
const expectedFresh = { automations: [newAutomation], runs: [newRun], loading: false, error: null }
beforeEach(() => {
	automations = []; runs = []; listeners = new Set(); calls = []
	store = createStore()
	Object.assign(window, { devo: {
		automation: {
			list: () => { calls.push("fetch"); return new Promise((resolve, reject) => automations.push({ resolve, reject })) },
			listRuns: () => new Promise((resolve, reject) => runs.push({ resolve, reject })),
		},
		onAutomationRunsUpdated: (cb: () => void) => { calls.push("subscribe"); listeners.add(cb); return () => listeners.delete(cb) },
	} })
	root = createRoot(document.createElement("div"))
})
afterEach(async () => { await act(async () => root.unmount()) })
test("subscribes before fetching and loads a complete snapshot", async () => {
	await render()
	expect(calls).toEqual(["subscribe", "fetch"])
	await settle(0, true)
	expect(snapshot()).toEqual(expectedFresh)
})
test("push refresh supersedes a slower initial snapshot", async () => {
	await render()
	await act(async () => { for (const cb of listeners) cb() })
	await settle(1, true)
	await settle(0, false)
	expect(snapshot()).toEqual(expectedFresh)
})
test("manual refresh supersedes a previous request", async () => {
	await render()
	const pending = result.refetch()
	await settle(1, true); await pending
	await settle(0, false)
	expect(snapshot()).toEqual(expectedFresh)
})
test("stale failure cannot replace success with an error", async () => {
	await render(); const pending = result.refetch()
	await settle(1, true); await pending
	await act(async () => automations[0].reject(new Error("old failure")))
	expect(snapshot()).toEqual(expectedFresh)
})
test("stale completion cannot finish loading for a pending newer read", async () => {
	await render(); const pending = result.refetch()
	await settle(0, false)
	expect(snapshot()).toEqual({ automations: [], runs: [], loading: true, error: null })
	await settle(1, true); await pending
	expect(snapshot()).toEqual(expectedFresh)
})
test("latest failure remains visible until a later successful refresh", async () => {
	await render(); const pending = result.refetch()
	await act(async () => { runs[1].reject(new Error("latest failure")); await pending })
	await settle(0, false)
	expect(snapshot()).toEqual({ automations: [], runs: [], loading: false, error: "latest failure" })
	const retry = result.refetch(); await settle(2, true); await retry
	expect(snapshot()).toEqual(expectedFresh)
})
test("StrictMode disposed request cannot overwrite the current effect", async () => {
	await render(true)
	expect(listeners.size).toBe(1)
	await settle(1, true); await settle(0, false)
	expect(snapshot()).toEqual(expectedFresh)
})
test("unmount prevents global atom writes and removes subscription", async () => {
	await render(); const refetch = result.refetch
	await act(async () => root.render(null))
	expect(listeners.size).toBe(0)
	await settle(0, true)
	void refetch()
	expect({ automations: store.get(automationsAtom), runs: store.get(automationRunsAtom), requests: automations.length })
		.toEqual({ automations: [], runs: [], requests: 1 })
})
