import { afterEach, expect, mock, test } from "bun:test"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { RuleFileInfo } from "../../../preload/api"
import { desktopFoldersAtom } from "../../atoms/desktop-folders"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const existing: RuleFileInfo = {
	path: "/existing/CLAUDE.md",
	name: "CLAUDE.md",
	directory: "/existing",
	scope: "project",
}
const created: RuleFileInfo = {
	path: "/missing/AGENTS.md",
	name: "AGENTS.md",
	directory: "/missing",
	scope: "project",
}
const list = mock(async (_directories: string[]): Promise<RuleFileInfo[]> => [existing])
const create = mock(async (_directory: string) => created)
const open = mock(async (_path: string) => {})
Object.assign(dom, { devo: { rules: { list, create, open } } })
const { createRoot } = await import("react-dom/client")
const { RuleSettings } = await import("./rule-settings")
let root: Root | undefined
let mount: HTMLDivElement
let store = createStore()
let query = new QueryClient({ defaultOptions: { queries: { retry: false } } })
function deferred<T>() {
	let resolve!: (value: T) => void
	const promise = new Promise<T>((yes) => {
		resolve = yes
	})
	return { promise, resolve }
}
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
		store.set(
			desktopFoldersAtom,
			["/existing", "/missing"].map((directory) => ({
				id: directory,
				directory,
				addedAt: 0,
			})),
		)
	}
	await act(async () =>
		root?.render(
			<Provider store={store}>
				<QueryClientProvider client={query}>
					<RuleSettings embedded searchQuery={searchQuery} />
				</QueryClientProvider>
			</Provider>,
		),
	)
	await flush()
}
function buttons(label: string) {
	return [...mount.querySelectorAll("button")].filter((button) => button.textContent === label)
}
async function click(label: string) {
	const button = buttons(label)[0]
	if (!button) throw new Error(`Missing ${label}`)
	await act(async () => button.click())
	await flush()
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	query.clear()
	query = new QueryClient({ defaultOptions: { queries: { retry: false } } })
	store = createStore()
	list.mockReset().mockResolvedValue([existing])
	create.mockReset().mockResolvedValue(created)
	open.mockClear()
})

test("loading is not evidence that project instructions are missing", async () => {
	const pending = deferred<RuleFileInfo[]>()
	list.mockImplementationOnce(() => pending.promise)
	await render()
	expect(mount.textContent).toContain("Looking for AGENTS.md files")
	expect(buttons("Create")).toHaveLength(0)
	await act(async () => pending.resolve([existing]))
	await flush()
	expect(buttons("Create")).toHaveLength(1)
})

test("failed listing and pending retry cannot offer creation; successful retry can", async () => {
	list.mockRejectedValueOnce(new Error("Cannot read folders"))
	await render()
	expect(mount.textContent).toContain("Failed to load rules")
	expect(buttons("Create")).toHaveLength(0)
	const pending = deferred<RuleFileInfo[]>()
	list.mockImplementationOnce(() => pending.promise)
	await click("Retry")
	expect(buttons("Create")).toHaveLength(0)
	await act(async () => pending.resolve([]))
	await flush()
	expect(buttons("Create")).toHaveLength(2)
	expect(mount.textContent).not.toContain("Failed to load rules")
})

test("failed refresh blocks creation even with cached files and recovers on retry", async () => {
	await render()
	list.mockRejectedValueOnce(new Error("Folder unavailable"))
	await act(async () => {
		await query.invalidateQueries({ queryKey: ["rule-files"] })
	})
	await flush()
	expect(mount.textContent).toContain("Folder unavailable")
	expect(buttons("Open")).toHaveLength(1)
	expect(buttons("Create")).toHaveLength(0)
	await click("Retry")
	expect(buttons("Create")).toHaveLength(1)
})

test("new directory listing must finish before showing missing folders", async () => {
	await render()
	const pending = deferred<RuleFileInfo[]>()
	list.mockImplementationOnce(() => pending.promise)
	await act(async () =>
		store.set(desktopFoldersAtom, [{ id: "new", directory: "/new", addedAt: 0 }]),
	)
	await flush()
	expect(buttons("Create")).toHaveLength(0)
	await act(async () => pending.resolve([]))
	await flush()
	expect(buttons("Create")).toHaveLength(1)
	expect(mount.textContent).toContain("/new")
})

test("successful list preserves existing files, search, open and create invalidation", async () => {
	await render()
	expect(list.mock.calls).toEqual([[["/existing", "/missing"]]])
	expect(buttons("Create")).toHaveLength(1)
	await click("Open")
	expect(open.mock.calls).toEqual([[existing.path]])
	await render("existing")
	expect(buttons("Create")).toHaveLength(0)
	await render("missing")
	expect(buttons("Open")).toHaveLength(0)
	expect(buttons("Create")).toHaveLength(1)
	list.mockResolvedValue([existing, created])
	await click("Create")
	expect(create.mock.calls).toEqual([["/missing"]])
	expect(buttons("Create")).toHaveLength(0)
	expect(buttons("Open")).toHaveLength(1)
})
