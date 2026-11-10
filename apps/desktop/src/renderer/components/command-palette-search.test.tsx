import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { Provider, useAtomValue } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { mockModeStorageAtom } from "../atoms/mock-mode"
import {
	projectPaginationFamily,
	removeSessionAtom,
	sessionFamily,
	sessionIdsAtom,
	setSessionsAtom,
} from "../atoms/sessions"
import { appStore } from "../atoms/store"
import type { Agent } from "../lib/types"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	localStorage: dom.localStorage,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	DocumentFragment: dom.DocumentFragment,
	MutationObserver: dom.MutationObserver,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
dom.HTMLElement.prototype.scrollIntoView = () => {}
const navigate = mock(() => {})
mock.module("@tanstack/react-router", () => ({
	useNavigate: () => navigate,
	useParams: () => ({}),
}))
mock.module("../hooks/use-agents", () => ({ useProjectList: () => [] }))
mock.module("../hooks/use-theme", () => ({
	useAvailableThemes: () => [],
	useCurrentTheme: () => ({ id: "default" }),
	useColorScheme: () => "system",
	useSetTheme: () => () => {},
	useSetColorScheme: () => () => {},
}))
let client: ReturnType<typeof createDevoClient>
const getClient = mock(() => client)
mock.module("../services/connection-manager", () => ({
	getBaseClient: getClient,
	reloadConfig: async () => {},
}))
const { CommandPalette } = await import("./command-palette")
const { createRoot } = await import("react-dom/client")
const requests: Array<{
	params: unknown
	resolve: (value: unknown) => void
	reject: (cause: Error) => void
}> = []
const transport: DevoNativeTransport = {
	async request(method, params) {
		if (method === "initialize")
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		if (method === "session/read")
			throw Object.assign(new Error("gone"), { code: "SessionNotFound" })
		if (method !== "session/list") throw new Error(`Unexpected ${method}`)
		return new Promise((resolve, reject) => requests.push({ params, resolve, reject }))
	},
	async respond() {},
	subscribe: () => () => {},
	connected: () => true,
}
let root: Root | undefined
let mount: HTMLElement
const close = mock(() => {})
function Harness({ open }: { open: boolean }) {
	const ids = useAtomValue(sessionIdsAtom)
	const agents = [...ids].map((id) => {
		const session = appStore.get(sessionFamily(id))!.session
		return {
			id,
			sessionId: id,
			name: session.title,
			project: "Fixture",
			projectSlug: "fixture",
			status: "idle",
			environment: "local",
		} as Agent
	})
	return <CommandPalette open={open} onOpenChange={close} agents={agents} />
}
async function render(open = true) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root!.render(
			<Provider store={appStore}>
				<Harness open={open} />
			</Provider>,
		),
	)
}
async function query(value: string) {
	await act(async () => {
		const input = document.querySelector<HTMLInputElement>("[cmdk-input]")!
		Object.getOwnPropertyDescriptor(dom.HTMLInputElement.prototype, "value")!.set!.call(
			input,
			value,
		)
		input.dispatchEvent(new dom.Event("input", { bubbles: true }) as unknown as Event)
	})
}
async function debounce() {
	await act(async () => {
		await new Promise((resolve) => setTimeout(resolve, 280))
	})
}
function native(id: string, title = "needle result") {
	return {
		id,
		version: 1,
		cwd: "/repo",
		title,
		createdAt: "2026-11-08T00:00:00Z",
		lastActivityAt: "2026-11-08T00:00:00Z",
		status: "idle",
		flags: ["waitingApproval"],
		archived: false,
		ephemeral: false,
		activity: "working",
		model: { provider: "test", model: "test" },
		settings: { permissionProfile: "default" },
		preview: "",
		queuedCount: 1,
		usage: {
			total: {
				inputTokens: 0,
				outputTokens: 0,
				cacheCreationInputTokens: 0,
				cacheReadInputTokens: 0,
				reasoningTokens: 0,
				totalTokens: 0,
				callCount: 0,
				meteredCallCount: 0,
				failedCallCount: 0,
				cancelledCallCount: 0,
			},
			byPurpose: [],
			updatedAt: "2026-11-08T00:00:00Z",
		},
	}
}
beforeEach(() => {
	client = createDevoClient({ transport })
	appStore.set(mockModeStorageAtom, false)
	appStore.set(serverConnectedAtom, true)
	appStore.set(serverUrlAtom, "stdio://local")
	appStore.set(activeServerIdAtom, "local")
})
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	client.dispose()
	requests.length = 0
	getClient.mockClear()
	navigate.mockClear()
	close.mockClear()
	for (const id of appStore.get(sessionIdsAtom)) appStore.set(removeSessionAtom, id)
})
test("disconnect discards pending Native search and reconnect reruns the unchanged query", async () => {
	await render()
	await query("needle")
	await debounce()
	expect(requests).toHaveLength(1)
	await act(async () => appStore.set(serverConnectedAtom, false))
	await act(async () => requests[0]!.resolve({ data: [native("stale")], nextCursor: null }))
	expect([...appStore.get(sessionIdsAtom)]).toEqual([])
	expect(document.body.textContent).toContain("Runtime disconnected")
	await act(async () => appStore.set(serverConnectedAtom, true))
	await debounce()
	expect(requests).toHaveLength(2)
	await act(async () => requests[1]!.resolve({ data: [native("fresh")], nextCursor: null }))
	expect([...appStore.get(sessionIdsAtom)]).toEqual(["fresh"])
})

test("debounces rapid queries, excludes short queries, and issues a bounded unscoped Native search", async () => {
	await render()
	await query("n")
	await debounce()
	expect(requests).toEqual([])
	await query("nee")
	await query("needle")
	await debounce()
	expect(requests.map((r) => r.params)).toEqual([{ cwds: [], search: "needle", limit: 50 }])
})

test("batched disconnect/reconnect invalidates old work even with the same runtime identity", async () => {
	await render()
	await query("needle")
	await debounce()
	await act(async () => {
		appStore.set(serverConnectedAtom, false)
		appStore.set(serverConnectedAtom, true)
	})
	await debounce()
	expect(requests).toHaveLength(2)
	await act(async () => requests[0]!.resolve({ data: [native("stale")], nextCursor: null }))
	expect([...appStore.get(sessionIdsAtom)]).toEqual([])
	await act(async () => requests[1]!.resolve({ data: [native("fresh")], nextCursor: null }))
	expect([...appStore.get(sessionIdsAtom)]).toEqual(["fresh"])
})

for (const change of ["query", "close", "unmount", "url", "identity", "demo"] as const) {
	test(`${change} discards pending search without seeding the SDK cache`, async () => {
		await render()
		await query("needle")
		await debounce()
		await act(async () => {
			if (change === "url") appStore.set(serverUrlAtom, "stdio://replacement")
			if (change === "identity") appStore.set(activeServerIdAtom, "replacement")
			if (change === "demo") appStore.set(mockModeStorageAtom, true)
			if (change === "unmount") {
				root!.unmount()
				root = undefined
			}
		})
		if (change === "query") await query("different")
		if (change === "close") await render(false)
		await act(async () => requests[0]!.resolve({ data: [native("stale")], nextCursor: null }))
		expect([...appStore.get(sessionIdsAtom)]).toEqual([])
		expect(await client.session.get({ sessionId: "stale" })).toEqual({ data: undefined })
	})
}

test("close/reopen starts a fresh query without sharing an abandoned SDK request", async () => {
	await render()
	await query("needle")
	await debounce()
	await render(false)
	await render()
	await query("needle")
	await debounce()
	expect(requests).toHaveLength(2)
	await act(async () => requests[1]!.resolve({ data: [native("fresh")], nextCursor: null }))
	await act(async () => requests[0]!.resolve({ data: [native("stale")], nextCursor: null }))
	expect([...appStore.get(sessionIdsAtom)]).toEqual(["fresh"])
})

test("offline and demo searches never acquire a Native client", async () => {
	appStore.set(serverConnectedAtom, false)
	await render()
	await query("needle")
	await debounce()
	expect(getClient.mock.calls).toEqual([])
	expect(document.body.textContent).toContain("Runtime disconnected")
	await act(async () => {
		appStore.set(mockModeStorageAtom, true)
		appStore.set(serverConnectedAtom, true)
	})
	await debounce()
	expect(getClient.mock.calls).toEqual([])
	expect(document.body.textContent).not.toContain("Runtime disconnected")
})

test("failure and loading stay visible beside local matches; Retry coalesces and preserves live status/pagination", async () => {
	appStore.set(setSessionsAtom, {
		sessions: [
			{ id: "loaded", title: "needle local", directory: "/repo", time: { created: 1, updated: 1 } },
		],
		statuses: { loaded: { type: "busy" } },
		directory: "/repo",
	})
	const pagination = { loaded: true, currentLimit: 5, hasMore: true, loading: false }
	appStore.set(projectPaginationFamily("/repo"), pagination)
	await render()
	await query("needle")
	await debounce()
	expect(document.body.textContent).toContain("Searching sessions…")
	expect(document.querySelector('[cmdk-item][data-value="session:loaded"]')).not.toBeNull()
	await act(async () => requests[0]!.reject(new Error("unavailable")))
	expect(document.body.textContent).toContain("Session search failed")
	const retry = document.querySelector<HTMLButtonElement>(
		'button[aria-label="Retry session search"]',
	)!
	expect(retry).not.toBeNull()
	await act(async () => {
		retry.click()
		retry.click()
		retry.click()
	})
	expect(requests).toHaveLength(2)
	expect(document.body.textContent).toContain("Searching sessions…")
	await act(async () =>
		requests[1]!.resolve({ data: [native("loaded"), native("found")], nextCursor: "more" }),
	)
	expect([...appStore.get(sessionIdsAtom)]).toEqual(["loaded", "found"])
	expect(appStore.get(sessionFamily("loaded"))?.status).toEqual({ type: "busy" })
	expect(appStore.get(projectPaginationFamily("/repo"))).toEqual(pagination)
	expect(document.body.textContent).not.toContain("Session search failed")
	expect(requests).toHaveLength(2)
	// Same title does not collapse identity; click the precise cmdk session row.
	const row = document.querySelector<HTMLElement>('[cmdk-item][data-value="session:found"]')!
	await act(async () => row.click())
	expect(navigate.mock.calls).toEqual([
		[
			{
				to: "/project/$projectSlug/session/$sessionId",
				params: { projectSlug: "fixture", sessionId: "found" },
			},
		],
	])
	expect(close.mock.calls).toEqual([[false]])
})

test("older query completion cannot replace newer results or show a stale error", async () => {
	await render()
	await query("needle")
	await debounce()
	await query("different")
	await debounce()
	await act(async () =>
		requests[1]!.resolve({ data: [native("new", "different result")], nextCursor: null }),
	)
	await act(async () => requests[0]!.reject(new Error("old failure")))
	expect([...appStore.get(sessionIdsAtom)]).toEqual(["new"])
	expect(document.body.textContent).not.toContain("Session search failed")
	expect(document.body.textContent).not.toContain("Searching sessions…")
})

test("Retry keyboard activation cannot select a matching session and returns focus to search", async () => {
	appStore.set(setSessionsAtom, {
		sessions: [{ id: "loaded", title: "needle local", time: { created: 1, updated: 1 } }],
		statuses: {},
		directory: "",
	})
	await render()
	await query("needle")
	await debounce()
	await act(async () => requests[0]!.reject(new Error("unavailable")))
	const retry = document.querySelector<HTMLButtonElement>(
		'button[aria-label="Retry session search"]',
	)!
	await act(async () => {
		retry.focus()
		retry.dispatchEvent(new dom.KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
		// happy-dom does not synthesize the browser's button activation click.
		retry.click()
	})
	expect(navigate.mock.calls).toEqual([])
	expect(close.mock.calls).toEqual([])
	expect(document.activeElement).toBe(document.querySelector("[cmdk-input]"))
	expect(requests).toHaveLength(2)
})

test("a successful empty search shows no results rather than an error", async () => {
	await render()
	await query("zzzzzzzz")
	await debounce()
	await act(async () => requests[0]!.resolve({ data: [], nextCursor: null }))
	expect(document.querySelector("[cmdk-empty]")?.textContent).toBe("No results found.")
	expect(document.querySelector("output")).toBeNull()
})
