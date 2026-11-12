import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { Automation, AutomationRun } from "../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, scrollTo: () => {}, IS_REACT_ACT_ENVIRONMENT: true })
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
let selectedRunId: string | undefined
function Probe() { result = useAutomationData(selectedRunId); return <span>{String(result.loading)}</span> }
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
	selectedRunId = undefined
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

const originalSetInterval = globalThis.setInterval
const originalClearInterval = globalThis.clearInterval
function capturePolling() {
 const timers = new Map<number, { callback: () => void; delay: number }>()
 let id = 0
 globalThis.setInterval = ((callback: () => void, delay: number) => {
  timers.set(++id, { callback, delay }); return id
 }) as typeof setInterval
 globalThis.clearInterval = ((id: number) => { timers.delete(id) }) as typeof clearInterval
 return timers
}
afterEach(() => {
 globalThis.setInterval = originalSetInterval
 globalThis.clearInterval = originalClearInterval
})
test("selected startup run accelerates the single gateway and session arrival restores cadence", async () => {
 const timers = capturePolling()
 selectedRunId = "r"
 store.set(automationRunsAtom, [oldRun])
 await render()
 expect([...timers.values()].map(t => t.delay)).toEqual([2000])
 await act(async () => { for (const t of timers.values()) t.callback() })
 expect(runs.length).toBe(1)
 await settle(0, false)
 await act(async () => { for (const t of timers.values()) t.callback() })
 const live = { ...oldRun, sessionId: "live-session" }
 await act(async () => { automations[1].resolve([newAutomation]); runs[1].resolve([live]) })
 expect(store.get(automationRunsAtom)).toEqual([live])
 expect([...timers.values()].map(t => t.delay)).toEqual([30000])
})
test("push supersedes fast poll without reverting the ready session", async () => {
 const timers = capturePolling()
 selectedRunId = "r"; store.set(automationRunsAtom, [oldRun])
 await render(); await settle(0, false)
 await act(async () => { for (const t of timers.values()) t.callback() })
 await act(async () => { for (const cb of listeners) cb() })
 await settle(2, true); await settle(1, false)
 expect(snapshot()).toEqual(expectedFresh)
})
test("switching away from startup run removes the fast timer without refetching or resubscribing", async () => {
 const timers = capturePolling()
 selectedRunId = "r"; store.set(automationRunsAtom, [oldRun])
 await render(); selectedRunId = "other"; await render()
 expect([...timers.values()].map(t => t.delay)).toEqual([30000])
 expect(calls).toEqual(["subscribe", "fetch"])
})
test("StrictMode and unmount leave no active fast timer or stale writer", async () => {
 const timers = capturePolling()
 selectedRunId = "r"; store.set(automationRunsAtom, [oldRun])
 await render(true)
 expect([...timers.values()].map(t => t.delay)).toEqual([2000])
 await act(async () => root.render(null))
 expect(timers.size).toBe(0)
 await settle(1, true)
 expect(store.get(automationRunsAtom)).toEqual([oldRun])
})

test("parent route gateway sees the selected nested run and restores normal polling on navigation", async () => {
 const { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useParams } = await import("@tanstack/react-router")
 const timers = capturePolling()
 store.set(automationRunsAtom, [oldRun])
 function Page() {
  const { runId } = useParams({ strict: false }) as { runId?: string }
  result = useAutomationData(runId)
  return <Outlet />
 }
 const rootRoute = createRootRoute()
 const page = createRoute({ getParentRoute: () => rootRoute, path: "/automations", component: Page })
 const detail = createRoute({ getParentRoute: () => page, path: "$automationId/runs/$runId", component: () => <span>run</span> })
 const index = createRoute({ getParentRoute: () => page, path: "/", component: () => <span>inbox</span> })
 const router = createRouter({ routeTree: rootRoute.addChildren([page.addChildren([detail, index])]), history: createMemoryHistory({ initialEntries: ["/automations/a/runs/r"] }) })
 await act(async () => { root.render(<Provider store={store}><RouterProvider router={router} /></Provider>) })
 expect([...timers.values()].map(t => t.delay)).toEqual([2000])
 await act(async () => { await router.navigate({ to: "/automations" }) })
 expect([...timers.values()].map(t => t.delay)).toEqual([30000])
 expect(runs.length).toBe(1)
})
test("slow requests survive multiple fast poll ticks instead of starving", async () => {
 const timers = capturePolling()
 selectedRunId = "r"; store.set(automationRunsAtom, [oldRun])
 await render()
 await act(async () => { for (let i = 0; i < 5; i++) for (const t of timers.values()) t.callback() })
 expect(runs.length).toBe(1)
 await settle(0, true)
 expect(snapshot()).toEqual(expectedFresh)
 await act(async () => { for (const t of timers.values()) t.callback() })
 expect(runs.length).toBe(2)
})
