import { afterEach, expect, mock, test } from "bun:test"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { activeServerIdAtom, serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"

const dom = new Window({ url: "http://localhost", width: 1188, height: 848 })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	ShadowRoot: dom.ShadowRoot,
	Event: dom.Event,
	MouseEvent: dom.MouseEvent,
	KeyboardEvent: dom.KeyboardEvent,
	MutationObserver: dom.MutationObserver,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})

let enabled = true
const write = mock(async (params: { path: string; enabled: boolean }) => {
	enabled = params.enabled
})
const list = mock(async () => ({
	data: [{ id: "review", name: "Review", path: "/skills/review", enabled }],
}))
let client = { app: { skills: list, setSkillEnabled: write } }
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => client,
	getProjectClient: () => client,
}))
const { createRoot } = await import("react-dom/client")
const { SkillSettings } = await import("./skill-settings")
let root: Root | undefined
let mount: HTMLElement
let store = createStore()
let query = new QueryClient({
	defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
})
async function flush() {
	await act(async () => {
		await new Promise((resolve) => setTimeout(resolve, 20))
	})
}
async function render(searchQuery = "") {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
		store.set(serverConnectedAtom, true)
	}
	await act(async () =>
		root?.render(
			<Provider store={store}>
				<QueryClientProvider client={query}>
					<SkillSettings embedded searchQuery={searchQuery} />
				</QueryClientProvider>
			</Provider>,
		),
	)
	await flush()
}
function control() {
	return document.querySelector('[role="switch"]') as HTMLButtonElement
}
function retry() {
	const button = [...document.querySelectorAll("button")].find(
		(button) => button.textContent === "Retry",
	)
	if (!button) throw new Error("Missing Retry button")
	return button
}
async function click(button: HTMLElement) {
	await act(async () => button.click())
	await flush()
}
function deferred() {
	let resolve!: () => void
	let reject!: (error: Error) => void
	const promise = new Promise<void>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	query.clear()
	store = createStore()
	query = new QueryClient({
		defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
	})
	enabled = true
	write.mockReset().mockImplementation(async (params) => {
		enabled = params.enabled
	})
	list.mockClear()
	client = { app: { skills: list, setSkillEnabled: write } }
})
test("failed write preserves saved state and retry intent through search filtering", async () => {
	query.invalidateQueries = mock(query.invalidateQueries.bind(query))
	write.mockImplementationOnce(async () => {
		throw new Error("skill config change blocked by hook: read-only")
	})
	await render()
	await click(control())
	expect(document.querySelector('[role="alert"]')?.textContent).toContain(
		"Failed to disable Review",
	)
	expect(document.body.textContent).toContain("skill config change blocked by hook: read-only")
	expect(control().getAttribute("aria-checked")).toBe("true")
	expect(query.invalidateQueries).not.toHaveBeenCalled()
	await render("no match")
	expect(document.body.textContent).toContain("No matching skills")
	await click(retry())
	expect(write.mock.calls).toEqual([
		[{ path: "/skills/review", enabled: false }],
		[{ path: "/skills/review", enabled: false }],
	])
	expect(document.querySelector('[role="alert"]')).toBeNull()
	expect(query.invalidateQueries).toHaveBeenCalledTimes(2)
	await render()
	expect(control().getAttribute("aria-checked")).toBe("false")
})
test("pending write prevents repeated clicks", async () => {
	const pending = deferred()
	write.mockImplementationOnce(() => pending.promise)
	await render()
	const button = control()
	await act(async () => {
		button.click()
		button.click()
	})
	await flush()
	expect(write).toHaveBeenCalledTimes(1)
	expect(control().hasAttribute("data-disabled")).toBe(true)
	await act(async () => pending.resolve())
	await flush()
	expect(control().hasAttribute("data-disabled")).toBe(false)
})
test("offline cached switch and pre-render clicks cannot write", async () => {
	await render()
	const button = control()
	await act(async () => {
		store.set(serverConnectedAtom, false)
		button.click()
	})
	await flush()
	expect(write).not.toHaveBeenCalled()
	expect(control().hasAttribute("data-disabled")).toBe(true)
	expect(document.body.textContent).toContain("Reconnect to change skills.")
	await act(async () => store.set(serverConnectedAtom, true))
	await flush()
	await click(control())
	expect(write).toHaveBeenCalledTimes(1)
})
test("dispatch resolves the current client", async () => {
	await render()
	const freshWrite = mock(async (_params: { path: string; enabled: boolean }) => {})
	client = { app: { skills: list, setSkillEnabled: freshWrite } }
	await click(control())
	expect(write).not.toHaveBeenCalled()
	expect(freshWrite).toHaveBeenCalledTimes(1)
})
test("retry is disabled offline and recovers after reconnect", async () => {
	write.mockImplementationOnce(async () => {
		throw new Error("Disconnected")
	})
	await render()
	await click(control())
	await act(async () => store.set(serverConnectedAtom, false))
	await flush()
	expect(retry().disabled).toBe(true)
	await click(retry())
	expect(write).toHaveBeenCalledTimes(1)
	await act(async () => store.set(serverConnectedAtom, true))
	await flush()
	await click(retry())
	expect(write).toHaveBeenCalledTimes(2)
	expect(document.querySelector('[role="alert"]')).toBeNull()
})
test("late failures do not attach retry to another project or server", async () => {
	const pending = deferred()
	write.mockImplementationOnce(() => pending.promise)
	await render()
	await click(control())
	await act(async () =>
		store.set(discoveryAtom, {
			...store.get(discoveryAtom),
			projects: [{ id: "b", worktree: "/project-b" }] as never,
		}),
	)
	await flush()
	await act(async () => pending.reject(new Error("Old project failed")))
	await flush()
	expect(document.querySelector('[role="alert"]')).toBeNull()
	expect(control().hasAttribute("data-disabled")).toBe(false)
	write.mockImplementationOnce(async () => {
		throw new Error("Project B failed")
	})
	await click(control())
	expect(document.querySelector('[role="alert"]')).not.toBeNull()
	await act(async () => store.set(activeServerIdAtom, "another-server"))
	await flush()
	expect(document.querySelector('[role="alert"]')).toBeNull()
})
test("unmount and reopen discard obsolete failure", async () => {
	const pending = deferred()
	write.mockImplementationOnce(() => pending.promise)
	await render()
	await click(control())
	await act(async () => root?.unmount())
	root = undefined
	mount.remove()
	await act(async () => pending.reject(new Error("Closed settings")))
	await render()
	expect(document.querySelector('[role="alert"]')).toBeNull()
})

test("successful enable keeps controls pending until skill queries finish refreshing", async () => {
	enabled = false
	await render()
	const refreshed = deferred()
	const original = query.invalidateQueries.bind(query)
	query.invalidateQueries = mock(async (options) => {
		await refreshed.promise
		return original(options)
	})
	await click(control())
	expect(write.mock.calls).toEqual([[{ path: "/skills/review", enabled: true }]])
	expect(control().hasAttribute("data-disabled")).toBe(true)
	await click(control())
	expect(write).toHaveBeenCalledTimes(1)
	await act(async () => refreshed.resolve())
	await flush()
	expect(control().hasAttribute("data-disabled")).toBe(false)
	expect(control().getAttribute("aria-checked")).toBe("true")
})

test("a workspace change before React renders cannot dispatch the old control", async () => {
	await render()
	const button = control()
	await act(async () => {
		store.set(activeServerIdAtom, "new-server")
		button.click()
	})
	await flush()
	expect(write).not.toHaveBeenCalled()
	expect(document.querySelector('[role="alert"]')).toBeNull()
})

test("server changes do not reuse fresh cached skills at the same directory", async () => {
	query.setDefaultOptions({ queries: { retry: false, staleTime: Infinity } })
	await render()
	expect(control().getAttribute("aria-checked")).toBe("true")
	enabled = false
	await act(async () => store.set(activeServerIdAtom, "server-b"))
	await flush()
	expect(control().getAttribute("aria-checked")).toBe("false")
	expect(list).toHaveBeenCalledTimes(2)
})

test("a cold offline view does not claim that no skills were discovered", async () => {
	await render()
	await act(async () => {
		store.set(serverConnectedAtom, false)
		query.clear()
	})
	await flush()
	expect(document.body.textContent).toContain("Runtime disconnected")
	expect(document.body.textContent).not.toContain("No skills discovered")
})
