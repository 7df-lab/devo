import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import {
	ReferenceSearchSession,
	type ReferenceSearchSnapshot,
} from "../../../packages/devo-ai-sdk/src/v2/reference-search-session"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { appStore } from "../atoms/store"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	Event: dom.Event,
	IS_REACT_ACT_ENVIRONMENT: true,
})
let requests: Array<{
	method: string
	params: any
	resolve: (value: unknown) => void
}>
let search: ReferenceSearchSession
let client: any
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => client,
}))
const { useReferenceSearch } = await import("./use-reference-search")
const { createRoot } = await import("react-dom/client")
let root: Root
let mount: HTMLElement
let result: ReturnType<typeof useReferenceSearch>
function Harness({ query, open, directory }: { query: string; open: boolean; directory: string }) {
	result = useReferenceSearch(directory, query, open)
	return (
		<div>
			{result.results.map((row) => (
				<button key={row.insert_text}>{row.display_name}</button>
			))}
		</div>
	)
}
async function render(query = "first", open = true, directory = "/repo") {
	await act(async () =>
		root.render(
			<Provider store={appStore}>
				<Harness {...{ query, open, directory }} />
			</Provider>,
		),
	)
}
async function debounce() {
	await act(async () => {
		await new Promise((resolve) => setTimeout(resolve, 170))
	})
}
async function complete(index: number, id: string, query: string) {
	await act(async () =>
		requests[index].resolve({
			snapshot: {
				search_id: id,
				query,
				results: [{ kind: "file", display_name: query, insert_text: query }],
				total_file_match_count: 1,
				scanned_file_count: 1,
				file_search_complete: true,
			} satisfies ReferenceSearchSnapshot,
		}),
	)
}
beforeEach(() => {
	requests = []
	search = new ReferenceSearchSession(
		(method, params) => new Promise((resolve) => requests.push({ method, params, resolve })),
		"/repo",
	)
	client = {
		referenceSearch: {
			subscribe: (listener: any) => search.subscribe(listener),
			startOrUpdate: ({ query }: { query: string }) => search.startOrUpdate(query),
			cancel: () => search.cancel(),
			getState: () => search.getState(),
		},
	}
	appStore.set(serverConnectedAtom, true)
	appStore.set(serverUrlAtom, "one")
	appStore.set(activeServerIdAtom, "one")
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
})
afterEach(async () => {
	await act(async () => root.unmount())
	mount.remove()
})

test("new query immediately removes selectable old rows during debounce", async () => {
	await render()
	await debounce()
	await complete(0, "one", "first")
	expect(mount.textContent).toBe("first")
	await render("second")
	expect(mount.querySelectorAll("button").length).toBe(0)
	expect(result.isLoading).toBe(true)
	await debounce()
	await complete(2, "two", "second")
	expect(mount.textContent).toBe("second")
})
test("closing then reopening the same query does not reuse old rows", async () => {
	await render()
	await debounce()
	await complete(0, "one", "first")
	await render("first", false)
	await render()
	expect(result.results).toEqual([])
	expect(result.isLoading).toBe(true)
})
test("directory A to B to A does not reuse a previous visit", async () => {
	await render()
	await debounce()
	await complete(0, "one", "first")
	await render("first", true, "/other")
	await render()
	expect(result.results).toEqual([])
})
test("closing before response and reopening same query rejects old response", async () => {
	await render()
	await debounce()
	await render("first", false)
	await render()
	await debounce()
	await complete(0, "old", "first")
	expect(result.results).toEqual([])
	await complete(1, "new", "first")
	expect(mount.textContent).toBe("first")
	expect(search.getState().snapshot?.search_id).toBe("new")
})
test("batched disconnect/reconnect invalidates old pending response", async () => {
	await render()
	await debounce()
	await act(async () => {
		appStore.set(serverConnectedAtom, false)
		appStore.set(serverConnectedAtom, true)
	})
	await complete(0, "old", "first")
	expect(result.results).toEqual([])
	await debounce()
	expect(requests.filter((r) => r.method === "search/start")).toHaveLength(2)
})
test("old search failure cannot replace newer query results", async () => {
	await render()
	await debounce()
	await render("second")
	await debounce()
	await complete(1, "new", "second")
	await act(async () =>
		search.handleNotification("search/failed", {
			search_id: "old",
			query: "first",
			message: "old failure",
		}),
	)
	expect(result.error).toBe(null)
	expect(mount.textContent).toBe("second")
})
