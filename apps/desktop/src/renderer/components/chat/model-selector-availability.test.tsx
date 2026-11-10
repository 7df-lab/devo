import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { QueryClientProvider } from "@tanstack/react-query"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../../atoms/connection"
import { mockModeStorageAtom } from "../../atoms/mock-mode"
import { appStore } from "../../atoms/store"
import { queryClient } from "../../lib/query-client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	Event: dom.Event,
	MouseEvent: dom.MouseEvent,
	KeyboardEvent: dom.KeyboardEvent,
	MutationObserver: dom.MutationObserver,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/searchable-list-popover", () => ({
	SearchableListPopover: wrap,
	SearchableListPopoverContent: wrap,
	SearchableListPopoverTrigger: wrap,
	SearchableListPopoverEmpty: wrap,
	SearchableListPopoverList: wrap,
	SearchableListPopoverSearch: () => <input />,
	useSearchableListPopoverSearch: () => "",
}))
mock.module("@devo/ui/hooks/use-mobile", () => ({ useIsMobile: () => false }))
mock.module("@tanstack/react-router", () => ({ useNavigate: () => () => {} }))
mock.module("../../hooks/use-model-directory", () => ({
	useModelDirectory: () => ({
		data: { models: [], credentials: [] },
		loading: false,
		refreshing: false,
		offline: false,
		error: null,
		refreshCatalog: refresh,
	}),
}))
const refresh = () => {}
mock.module("../settings/template-connect-dialog", () => ({
	TemplateConnectDialog: ({ onConnected }: { onConnected: () => void }) => (
		<button type="button" onClick={onConnected}>
			Complete setup
		</button>
	),
}))
mock.module("../settings/connect-provider-dialog", () => ({ ConnectProviderDialog: () => null }))
const catalog = {
	providers: [
		{
			id: "local",
			name: "Local",
			enabled: true,
			baseUrl: "http://127.0.0.1:1/v1",
			wireApis: ["anthropic_messages"],
			models: { unusable: { name: "Unusable" } },
		},
	],
	connectedProviderIds: ["local"],
	templateProviderIds: [],
	connectionModels: {},
}
function preferences(value?: string) {
	return {
		preferences: {
			model: value ?? "local/unusable",
			availableModels: value ? [{ value, label: value }] : [],
			availableEfforts: [],
		},
	}
}
let read: (params: unknown) => Promise<ReturnType<typeof preferences>>
const requests: unknown[] = []
let transport: DevoNativeTransport
const clients = new Map<string, ReturnType<typeof createDevoClient>>()
function client(directory?: string) {
	const key = directory ?? ""
	let current = clients.get(key)
	if (!current) {
		current = createDevoClient({ directory, transport })
		clients.set(key, current)
	}
	return current
}
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => client(),
	getProjectClient: (directory: string) => client(directory),
	invalidateConfigOptionCaches: () => {
		for (const current of clients.values()) current.invalidateConfigOptionCaches()
	},
}))
mock.module("../../services/backend", () => ({
	fetchGitBranches: async () => [],
	fetchModelState: async () => ({}),
	isElectron: () => false,
	updateModelRecent: async () => {},
}))
const { createRoot } = await import("react-dom/client")
const { ModelSelector } = await import("./model-selector")
const { useProviders } = await import("../../hooks/use-devo-data")
function Picker({ directory, open }: { directory: string | null; open?: boolean }) {
	return (
		<ModelSelector
			presentation={open === undefined ? "inline" : "popover"}
			open={open}
			directory={directory}
			effectiveModel={null}
			hasOverride={false}
			onSelectModel={selected}
		/>
	)
}
function ProviderDependentToolbar({ directory }: { directory: string | null }) {
	const { data } = useProviders(directory)
	return data ? <Picker directory={directory} /> : <span>Toolbar loading</span>
}
let root: Root | undefined
let mount: HTMLElement
const selected = mock((_value: unknown) => {})
async function flush() {
	await act(async () => {
		await new Promise((resolve) => setTimeout(resolve, 0))
	})
}
async function render(
	directory: string | null = "/workspace/project",
	toolbar = false,
	open?: boolean,
) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root?.render(
			<Provider store={appStore}>
				<QueryClientProvider client={queryClient}>
					{toolbar ? (
						<ProviderDependentToolbar directory={directory} />
					) : (
						<Picker directory={directory} open={open} />
					)}
				</QueryClientProvider>
			</Provider>,
		),
	)
	await flush()
}
async function click(label: string) {
	const button = [...mount.querySelectorAll("button")].find((entry) => entry.textContent === label)
	if (!button) throw Error(`Missing ${label}: ${mount.textContent}`)
	await act(async () => button.click())
	await flush()
}
function rows() {
	return [...mount.querySelectorAll<HTMLButtonElement>("button[data-model-option]")]
}
function deferred() {
	let resolve!: (value: ReturnType<typeof preferences>) => void
	const promise = new Promise<ReturnType<typeof preferences>>((yes) => {
		resolve = yes
	})
	return { promise, resolve }
}
beforeEach(() => {
	appStore.set(mockModeStorageAtom, false)
	queryClient.setDefaultOptions({
		queries: { retry: false, gcTime: 0, refetchOnWindowFocus: false },
	})
	appStore.set(serverConnectedAtom, true)
	appStore.set(activeServerIdAtom, "one")
	appStore.set(serverUrlAtom, "server-one")
	read = async () => preferences()
	transport = {
		connected: () => appStore.get(serverConnectedAtom),
		subscribe: () => () => {},
		respond: async () => {},
		request: async (method, params) => {
			if (method === "initialize")
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			if (method === "provider/list") return catalog
			if (method !== "model/preferences/read") throw Error(method)
			requests.push(params)
			return read(params)
		},
	}
})
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	queryClient.clear()
	appStore.set(mockModeStorageAtom, false)
	clients.clear()
	requests.length = 0
	selected.mockClear()
})

test("real SDK and picker do not resurrect a Native-filtered configured route", async () => {
	await render()
	expect(rows()).toEqual([])
	await click("All models")
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual([
		"Unusable, Local, connect provider",
	])
	await act(async () => rows()[0].click())
	expect(selected).not.toHaveBeenCalled()
	expect(mount.textContent).toContain("Complete setup")
	await click("Complete setup")
	expect(selected).not.toHaveBeenCalled()
	expect(requests.length).toBeGreaterThan(1)
})

test("workspace-only routes remain selectable without global catalog entries", async () => {
	read = async () => preferences("workspace-only/tiny")
	await render()
	expect(rows()).toHaveLength(1)
	await act(async () => rows()[0].click())
	expect(selected.mock.calls).toEqual([[{ providerID: "session", modelID: "workspace-only/tiny" }]])
})

test("Native loading is distinct from empty availability and prevents premature setup clicks", async () => {
	const pending = deferred()
	read = () => pending.promise
	await render()
	expect(mount.textContent).toContain("Loading available models…")
	await click("All models")
	expect(rows()[0].disabled).toBe(true)
	await act(async () => rows()[0].click())
	expect(mount.textContent).not.toContain("Complete setup")
	await act(async () => pending.resolve(preferences()))
	await flush()
	expect(rows()[0].disabled).toBe(false)
})

test("disconnect discards an old pending read and reconnect obtains fresh availability", async () => {
	const old = deferred()
	read = () => old.promise
	await render()
	await act(async () => appStore.set(serverConnectedAtom, false))
	await flush()
	expect(mount.textContent).toContain("Connect to the server")
	read = async () => preferences()
	await act(async () => appStore.set(serverConnectedAtom, true))
	await flush()
	await act(async () => old.resolve(preferences("obsolete/model")))
	await flush()
	expect(rows()).toEqual([])
	expect(requests).toHaveLength(2)
})

test("Native read failure hides cached choices and Retry restores only fresh choices", async () => {
	read = async () => preferences("workspace/first")
	await render()
	expect(rows()).toHaveLength(1)
	read = async () => {
		throw Error("availability failed")
	}
	await act(async () => {
		await queryClient.invalidateQueries({ queryKey: ["providers"] })
	})
	await flush()
	expect(rows()).toEqual([])
	expect(mount.textContent).toContain("Could not load model availability")
	read = async () => preferences("workspace/second")
	await click("Retry")
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual(["workspace/second, Session"])
})

test("switching workspaces cannot adopt a late response from the previous workspace", async () => {
	const old = deferred()
	read = async (params) =>
		(params as { cwd: string }).cwd === "/workspace/old" ? old.promise : preferences("new/tiny")
	await render("/workspace/old")
	await render("/workspace/new")
	await act(async () => old.resolve(preferences("old/tiny")))
	await flush()
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual(["new/tiny, Session"])
})

test("global Settings availability uses the Native process scope", async () => {
	read = async () => preferences("global/tiny")
	await render("")
	expect(requests).toEqual([{}])
	expect(rows()).toHaveLength(1)
})

test("a toolbar and its picker share availability without remounting during a refresh", async () => {
	read = async () => preferences("workspace/first")
	await render("/workspace/project", true)
	await flush()
	expect(rows()).toHaveLength(1)
	// Mounting the picker rechecks stale data once, without making its parent
	// lose data, unmount it, and start an endless remount/refetch loop.
	expect(requests).toHaveLength(2)
	const pending = deferred()
	read = () => pending.promise
	await act(async () => {
		void queryClient.invalidateQueries({ queryKey: ["providers"] })
	})
	await flush()
	expect(mount.textContent).toContain("Loading available models…")
	expect(mount.textContent).not.toContain("Toolbar loading")
	await act(async () => pending.resolve(preferences("workspace/second")))
	await flush()
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual(["workspace/second, Session"])
	expect(requests).toHaveLength(3)
})

test("server identity changes cannot adopt an old server's late availability", async () => {
	const old = deferred()
	read = () => old.promise
	await render()
	read = async () => preferences("new-server/tiny")
	await act(async () => {
		appStore.set(activeServerIdAtom, "two")
		appStore.set(serverUrlAtom, "server-two")
	})
	await flush()
	await act(async () => old.resolve(preferences("old-server/tiny")))
	await flush()
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual(["new-server/tiny, Session"])
	expect(requests).toHaveLength(2)
})

test("no selected project is distinct from global settings and performs no availability read", async () => {
	await render(null)
	expect(requests).toEqual([])
	expect(mount.textContent).toContain("Choose a project to load available models")
	await click("All models")
	expect(rows()[0].disabled).toBe(true)
})

test("reopening a mounted picker rechecks Native instead of offering its previous Ready choices", async () => {
	read = async () => preferences("workspace/previous")
	await render("/workspace/project", false, true)
	expect(rows()).toHaveLength(1)
	await render("/workspace/project", false, false)
	const fresh = deferred()
	read = () => fresh.promise
	await render("/workspace/project", false, true)
	expect(requests).toHaveLength(2)
	expect(rows()).toEqual([])
	expect(mount.textContent).toContain("Loading available models…")
	await act(async () => fresh.resolve(preferences()))
	await flush()
	expect(rows()).toEqual([])
	expect(selected).not.toHaveBeenCalled()
})

test("reopening cancels an unfinished first read even when no availability was cached", async () => {
	const old = deferred()
	read = () => old.promise
	await render("/workspace/project", false, true)
	await render("/workspace/project", false, false)
	read = async () => preferences()
	await render("/workspace/project", false, true)
	await act(async () => old.resolve(preferences("obsolete/model")))
	await flush()
	expect(requests).toHaveLength(2)
	expect(rows()).toEqual([])
})

test("a late reopen refresh cannot replace a new workspace or survive disconnect", async () => {
	read = async () => preferences("first/model")
	await render("/workspace/first", false, true)
	await render("/workspace/first", false, false)
	const old = deferred()
	read = () => old.promise
	await render("/workspace/first", false, true)
	read = async () => preferences("second/model")
	await render("/workspace/second", false, true)
	await act(async () => old.resolve(preferences("obsolete/model")))
	await flush()
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual(["second/model, Session"])
	await render("/workspace/second", false, false)
	await act(async () => appStore.set(serverConnectedAtom, false))
	await render("/workspace/second", false, true)
	expect(requests).toHaveLength(3)
	expect(rows()).toEqual([])
	expect(mount.textContent).toContain("Connect to the server")
	read = async () => preferences()
	await act(async () => appStore.set(serverConnectedAtom, true))
	await flush()
	expect(requests).toHaveLength(4)
	expect(rows()).toEqual([])
})

test("a failed reopen read cannot select cached choices and can retry", async () => {
	read = async () => preferences("workspace/previous")
	await render("/workspace/project", false, true)
	await render("/workspace/project", false, false)
	read = async () => {
		throw Error("Native read failed")
	}
	await render("/workspace/project", false, true)
	expect(rows()).toEqual([])
	expect(mount.textContent).toContain("Could not load model availability")
	read = async () => preferences("workspace/current")
	await click("Retry")
	expect(rows().map((row) => row.getAttribute("aria-label"))).toEqual([
		"workspace/current, Session",
	])
	expect(requests).toHaveLength(3)
})

test("reopening in mock mode does not wait for a disabled Native query", async () => {
	appStore.set(mockModeStorageAtom, true)
	await render("/workspace/project", false, true)
	await render("/workspace/project", false, false)
	await render("/workspace/project", false, true)
	expect(requests).toEqual([])
	expect(rows().length).toBeGreaterThan(0)
	expect(mount.textContent).not.toContain("Loading available models…")
})
