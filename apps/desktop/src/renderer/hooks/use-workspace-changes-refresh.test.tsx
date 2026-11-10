import { afterEach, expect, mock, test } from "bun:test"
import {
	createDevoClient,
	type WorkspaceChangeScope,
	type WorkspaceChangeView,
} from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { appStore } from "../atoms/store"
import {
	setWorkspaceChangesViewAtom,
	workspaceChangesKey,
	workspaceChangesStateFamily,
} from "../atoms/workspace-changes"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
let client: ReturnType<typeof createDevoClient>
mock.module("../services/connection-manager", () => ({ getProjectClient: () => client }))
const { useWorkspaceChanges } = await import("./use-workspace-changes")
const { createRoot } = await import("react-dom/client")
type Request = {
	params: { diffDetail: string; paths?: string[]; includeFileSides?: boolean }
	resolve: (result: { views: WorkspaceChangeView[] }) => void
	reject: (error: Error) => void
}
let requests: Request[] = []
let root: Root | undefined
let changes: ReturnType<typeof useWorkspaceChanges>
let sibling: ReturnType<typeof useWorkspaceChanges>
let directory = ""
let fixtureId = 0
let selectedScope: WorkspaceChangeScope = "uncommitted"
let hookOptions: { baseBranch?: string; ignoreWhitespace?: boolean } = {}
function view(paths = ["a.ts"], text?: string): WorkspaceChangeView {
	return {
		scope: "uncommitted",
		status: "ready",
		workspaceRoot: directory,
		coverage: "git_visible",
		attribution: "git_working_tree",
		changeSetStatus: "accumulating",
		files: paths.map((path) => ({
			path,
			status: "modified",
			additions: 1,
			deletions: 1,
			binary: false,
			diffTruncated: false,
			...(text === undefined ? {} : { oldText: "base\n", newText: `${text}\n` }),
		})),
		stats: { filesChanged: paths.length, additions: paths.length, deletions: paths.length },
		warnings: [],
		generatedAt: "2026-11-10T00:00:00Z",
		...(text === undefined
			? {}
			: {
					unifiedDiff: paths
						.map(
							(path) =>
								`diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1 +1 @@\n-base\n+${text}\n`,
						)
						.join(""),
				}),
	} as unknown as WorkspaceChangeView
}
function Probe({ second = false }: { second?: boolean }) {
	const result = useWorkspaceChanges(
		second ? "second-session" : "session",
		directory,
		selectedScope,
		{ prefetch: false, ...hookOptions },
	)
	if (second) sibling = result
	else changes = result
	return null
}
async function mount(two = false) {
	directory = `/refresh-fixture-${++fixtureId}`
	requests = []
	selectedScope = "uncommitted"
	hookOptions = {}
	client = createDevoClient({
		transport: {
			connected: () => true,
			subscribe: () => () => {},
			respond: async () => {},
			request: async (method, params) => {
				if (method === "initialize")
					return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
				if (method !== "workspace/changes/read") throw Error(`Unexpected RPC ${method}`)
				return new Promise<{ views: WorkspaceChangeView[] }>((resolve, reject) => {
					requests.push({ params: params as Request["params"], resolve, reject })
				})
			},
		},
	})
	appStore.set(setWorkspaceChangesViewAtom, {
		key: workspaceChangesKey({ cwd: directory, scope: "uncommitted" }),
		view: view(),
	})
	const element = document.createElement("div")
	document.body.append(element)
	root = createRoot(element)
	await act(async () =>
		root?.render(
			<Provider store={appStore}>
				<Probe />
				{two && <Probe second />}
			</Provider>,
		),
	)
}
async function respond(index: number, result: WorkspaceChangeView) {
	await act(async () => requests[index].resolve({ views: [result] }))
}
async function expand() {
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	await respond(0, view(["a.ts"], "old"))
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
})

test("Refresh replaces an expanded file list and reloads retained details even with identical statistics", async () => {
	await mount()
	await expand()
	await act(async () => {
		void changes.refetch()
	})
	await respond(1, view(["a.ts", "b.ts"]))
	expect(changes.view?.files.map((file) => file.path)).toEqual(["a.ts", "b.ts"])
	expect(requests.map(({ params }) => [params.diffDetail, params.paths])).toEqual([
		["full", ["a.ts"]],
		["summary", undefined],
		["full", ["a.ts"]],
	])
	await respond(2, view(["a.ts"], "fresh"))
	expect(changes.view?.files[0]?.newText).toBe("fresh\n")
	expect(changes.view?.unifiedDiff).toContain("+fresh")
	expect(changes.view?.unifiedDiff).not.toContain("+old")
	expect(changes.view?.files[1]?.newText).toBeUndefined()
})

test("Refresh removes vanished files and does not reload their details", async () => {
	await mount()
	await expand()
	await act(async () => {
		void changes.refetch()
	})
	const empty = view([])
	await respond(1, empty)
	expect(changes.view).toEqual(empty)
	expect(requests).toHaveLength(2)
})

test("an obsolete detail cannot restore a file removed by Refresh", async () => {
	await mount()
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	await act(async () => {
		void changes.refetch()
	})
	const empty = view([])
	await respond(1, empty)
	await respond(0, view(["a.ts"], "obsolete"))
	expect(changes.view).toEqual(empty)
})

test("duplicate Refresh and expand clicks coalesce; old detail settling cannot clear a new request", async () => {
	await mount()
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
		void changes.ensureFullPatches("a.ts")
		void changes.refetch()
		void changes.refetch()
	})
	expect(requests).toHaveLength(2)
	await respond(1, view())
	expect(requests).toHaveLength(3)
	await respond(0, view(["a.ts"], "obsolete"))
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	expect(requests).toHaveLength(3)
	await respond(2, view(["a.ts"], "fresh"))
	expect(changes.view?.files[0]?.newText).toBe("fresh\n")
})

test("Refresh invalidates details across hook instances sharing the workspace cache", async () => {
	await mount(true)
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
		void sibling.refetch()
	})
	await respond(1, view([]))
	await respond(0, view(["a.ts"], "obsolete"))
	expect(changes.view).toEqual(view([]))
	expect(sibling.view).toEqual(view([]))
})

test("identical summary metadata still invalidates cached sides and patches", async () => {
	await mount()
	await expand()
	await act(async () => {
		void changes.refetch()
	})
	await respond(1, view())
	expect(changes.view).toEqual(view())
	await respond(2, view(["a.ts"], "same-count-different-content"))
	expect(changes.view).toEqual(view(["a.ts"], "same-count-different-content"))
})

test("a failed Refresh preserves readable details and Retry replaces them", async () => {
	await mount()
	await expand()
	await act(async () => {
		void changes.refetch()
	})
	expect(changes.isRefreshing).toBe(true)
	expect(changes.view).toEqual(view(["a.ts"], "old"))
	await act(async () => requests[1].reject(Error("connection lost")))
	expect({ view: changes.view, loading: changes.loading, error: changes.error }).toEqual({
		view: view(["a.ts"], "old"),
		loading: false,
		error: "connection lost",
	})
	expect(requests).toHaveLength(2)
	await act(async () => {
		void changes.refetch()
	})
	await respond(2, view())
	await respond(3, view(["a.ts"], "fresh"))
	expect({ view: changes.view, error: changes.error }).toEqual({
		view: view(["a.ts"], "fresh"),
		error: null,
	})
})

test("unavailable sides are attempted once per Summary, then retried after Refresh", async () => {
	await mount()
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	await respond(0, view())
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	expect(requests).toHaveLength(1)
	await act(async () => {
		void changes.refetch()
	})
	await respond(1, view())
	expect(requests).toHaveLength(3)
	await respond(2, view())
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	expect(requests).toHaveLength(3)
})

test("an obsolete detail error cannot dismiss an active Refresh or overwrite its error", async () => {
	await mount()
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
		void changes.refetch()
	})
	await act(async () => requests[0].reject(Error("old detail failed")))
	expect({ loading: changes.loading, error: changes.error }).toEqual({ loading: true, error: null })
	await respond(1, view())
	await act(async () => requests[2].reject(Error("fresh detail failed")))
	expect({ loading: changes.loading, error: changes.error }).toEqual({
		loading: false,
		error: "fresh detail failed",
	})
	expect(requests).toHaveLength(3)
})

test("a file expanded during Refresh waits for the fresh Summary", async () => {
	await mount()
	await act(async () => {
		void changes.refetch()
		void changes.ensureFullPatches("a.ts")
	})
	expect(requests.map(({ params }) => params.diffDetail)).toEqual(["summary"])
	await respond(0, view())
	expect(requests[1]?.params.paths).toEqual(["a.ts"])
	await respond(1, view(["a.ts"], "fresh"))
	expect(changes.view).toEqual(view(["a.ts"], "fresh"))
})

test("Refresh invalidates an in-flight Expand all and reloads its retained paths", async () => {
	await mount()
	await act(async () => {
		void changes.ensureFullPatches()
		void changes.refetch()
	})
	await respond(1, view())
	expect(requests.map(({ params }) => [params.diffDetail, params.paths])).toEqual([
		["full", undefined],
		["summary", undefined],
		["full", ["a.ts"]],
	])
	await respond(2, view(["a.ts"], "fresh"))
	await respond(0, view(["a.ts", "removed.ts"], "obsolete"))
	expect(changes.view).toEqual(view(["a.ts"], "fresh"))
})

test("Refresh reloads details in bounded batches and stops obsolete batches", async () => {
	await mount()
	const paths = Array.from({ length: 9 }, (_, i) => `${i}.ts`)
	await act(async () =>
		appStore.set(setWorkspaceChangesViewAtom, {
			key: changes.key,
			view: view(paths),
		}),
	)
	await act(async () => {
		void changes.ensureFullPatches()
		void changes.refetch()
	})
	await respond(1, view(paths))
	expect(requests).toHaveLength(6)
	await act(async () => {
		void changes.refetch()
	})
	await respond(6, view([]))
	for (let i = 2; i < 6; i++) await respond(i, view([paths[i - 2]], "obsolete"))
	await respond(0, view(paths, "obsolete"))
	expect(requests).toHaveLength(7)
	expect(changes.view).toEqual(view([]))
})

test("a successful sibling detail cannot hide a failed row's Retry", async () => {
	await mount()
	await act(async () =>
		appStore.set(setWorkspaceChangesViewAtom, {
			key: changes.key,
			view: view(["a.ts", "b.ts"]),
		}),
	)
	await act(async () => {
		void changes.ensureFullPatches()
		void changes.refetch()
	})
	await respond(1, view(["a.ts", "b.ts"]))
	await act(async () => requests[2].reject(Error("a.ts failed")))
	await respond(3, view(["b.ts"], "fresh"))
	expect(changes.error).toBe("a.ts failed")
	expect(changes.view?.files[1]?.newText).toBe("fresh\n")
	await act(async () => {
		void changes.ensureFullPatches("a.ts")
	})
	await respond(4, view(["a.ts"], "retried"))
	expect(changes.error).toBeNull()
	expect(requests).toHaveLength(5)
	await respond(0, view(["a.ts", "b.ts"], "obsolete"))
	expect(changes.view?.files[0]?.newText).toBe("retried\n")
})

for (const partition of ["scope", "whitespace", "base branch"] as const) {
	test(`changing ${partition} isolates Refresh and its late details`, async () => {
		await mount()
		const oldKey = changes.key
		const oldView = view(["a.ts"], "old partition")
		await act(async () => {
			void changes.ensureFullPatches("a.ts")
		})
		if (partition === "scope") selectedScope = "staged"
		if (partition === "whitespace") hookOptions = { ignoreWhitespace: true }
		if (partition === "base branch") hookOptions = { baseBranch: "main" }
		const selectedView = { ...view(), scope: selectedScope }
		await act(async () =>
			appStore.set(setWorkspaceChangesViewAtom, {
				key: workspaceChangesKey({ cwd: directory, scope: selectedScope, ...hookOptions }),
				view: selectedView,
			}),
		)
		await act(async () =>
			root?.render(
				<Provider store={appStore}>
					<Probe />
				</Provider>,
			),
		)
		await act(async () => {
			void changes.refetch()
		})
		await respond(1, selectedView)
		await respond(0, oldView)
		expect(changes.view).toEqual(selectedView)
		expect(appStore.get(workspaceChangesStateFamily(oldKey)).view).toEqual(oldView)
		expect(requests).toHaveLength(2)
	})
}
