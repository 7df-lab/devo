import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { atom, createStore, Provider, useAtomValue } from "jotai"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost", width: 1188, height: 848 })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
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
const projects = atom([{ directory: "/a", name: "A" }])
mock.module("../../hooks/use-agents", () => ({ useProjectList: () => useAtomValue(projects) }))
const listWorktrees = mock(async (directory: string) => [`${directory}/tree`])
const removeWorktree = mock(async (_project: string, _directory: string) => {})
const resetWorktree = mock(async (_project: string, _directory: string) => {})
mock.module("../../services/worktree-service", () => ({
	listWorktrees,
	removeWorktree,
	resetWorktree,
}))
const { createRoot } = await import("react-dom/client")
const { WorktreeSettings } = await import("./worktree-settings")
let root: Root | undefined
let mount: HTMLDivElement
let store = createStore()
function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
async function render(strict = false) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	const content = (
		<Provider store={store}>
			<WorktreeSettings />
		</Provider>
	)
	await act(async () => root?.render(strict ? <StrictMode>{content}</StrictMode> : content))
}
async function project(directory: string) {
	await act(async () => store.set(projects, [{ directory, name: directory }]))
}
function button(label: string) {
	const result = [...document.querySelectorAll("button")].find((el) => el.textContent === label)
	if (!result) throw new Error(`Missing ${label}: ${document.body.textContent}`)
	return result
}
async function openRemove() {
	await act(async () =>
		document.querySelector<HTMLButtonElement>('[title="Remove worktree"]')?.click(),
	)
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	store = createStore()
	listWorktrees.mockReset().mockImplementation(async (directory) => [`${directory}/tree`])
	removeWorktree.mockReset().mockResolvedValue()
	resetWorktree.mockReset().mockResolvedValue()
})

test("an older project listing cannot replace the current project", async () => {
	const old = deferred<string[]>()
	listWorktrees.mockImplementationOnce(() => old.promise)
	await render()
	await project("/b")
	await act(async () => old.resolve(["/a/late"]))
	expect({
		current: mount.textContent?.includes("/b/tree"),
		old: mount.textContent?.includes("/a/late"),
	}).toEqual({ current: true, old: false })
})

test("an obsolete listing failure cannot hide current results or show stale errors", async () => {
	const old = deferred<string[]>()
	listWorktrees.mockImplementationOnce(() => old.promise)
	await render()
	await project("/b")
	await act(async () => old.reject(new Error("old project unavailable")))
	expect({
		current: mount.textContent?.includes("/b/tree"),
		error: mount.querySelector('[role="alert"]')?.textContent,
	}).toEqual({ current: true, error: undefined })
})

test("removing a project dismisses its unsubmitted worktree confirmation", async () => {
	await render()
	await openRemove()
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain("/a/tree")
	await project("/b")
	expect(Boolean(document.querySelector('[role="dialog"]'))).toBe(false)
	expect(removeWorktree.mock.calls).toEqual([])
})

test("an old removal cannot close a new project's confirmation or refresh its data", async () => {
	const removal = deferred<void>()
	removeWorktree.mockImplementationOnce(() => removal.promise)
	await render()
	await openRemove()
	await act(async () => button("Remove").click())
	await project("/b")
	await openRemove()
	const before = listWorktrees.mock.calls.length
	await act(async () => removal.resolve())
	expect({
		dialog: document.querySelector('[role="dialog"]')?.textContent?.includes("/b/tree"),
		reads: listWorktrees.mock.calls.length,
	}).toEqual({ dialog: true, reads: before })
})

for (const action of ["Remove", "Reset"]) {
	test(`${action} rejects duplicate clicks and remains pending across project revisits`, async () => {
		const mutation = deferred<void>()
		const service = action === "Remove" ? removeWorktree : resetWorktree
		service.mockImplementationOnce(() => mutation.promise)
		await render()
		await act(async () =>
			document
				.querySelector<HTMLButtonElement>(
					action === "Remove"
						? '[title="Remove worktree"]'
						: '[title="Reset worktree to default branch"]',
				)
				?.click(),
		)
		const confirm = button(action)
		const cancel = button("Cancel")
		await act(async () => {
			confirm.click()
			confirm.click()
			cancel.click()
		})
		expect(service.mock.calls).toEqual([["/a", "/a/tree"]])
		expect(button("Working…").disabled).toBe(true)
		await project("/b")
		await project("/a")
		const controls = [...mount.querySelectorAll<HTMLButtonElement>("button[title]")]
		expect(controls.map((control) => control.disabled)).toEqual([true, true])
		const reads = listWorktrees.mock.calls.length
		await act(async () => mutation.resolve())
		expect(controls.map((control) => control.disabled)).toEqual([false, false])
		expect(listWorktrees.mock.calls.length).toBe(reads)
	})

	test(`${action} errors after a project change cannot contaminate a new dialog`, async () => {
		const mutation = deferred<void>()
		const service = action === "Remove" ? removeWorktree : resetWorktree
		service.mockImplementationOnce(() => mutation.promise)
		await render()
		await act(async () =>
			document
				.querySelector<HTMLButtonElement>(
					action === "Remove"
						? '[title="Remove worktree"]'
						: '[title="Reset worktree to default branch"]',
				)
				?.click(),
		)
		await act(async () => button(action).click())
		await project("/b")
		await openRemove()
		await act(async () => mutation.reject(new Error("obsolete action failed")))
		expect({
			error: document.querySelector('[role="alert"]')?.textContent,
			dialog: document.querySelector('[role="dialog"]')?.textContent?.includes("/b/tree"),
		}).toEqual({ error: undefined, dialog: true })
	})
}

test("returning to a project does not revive its earlier listing", async () => {
	const old = deferred<string[]>()
	listWorktrees.mockImplementationOnce(() => old.promise)
	await render()
	await project("/b")
	await project("/a")
	await act(async () => old.resolve(["/a/obsolete"]))
	expect({
		current: mount.textContent?.includes("/a/tree"),
		old: mount.textContent?.includes("/a/obsolete"),
	}).toEqual({ current: true, old: false })
})

test("StrictMode cleanup invalidates the first mount's listing", async () => {
	const first = deferred<string[]>()
	listWorktrees.mockImplementationOnce(() => first.promise)
	await render(true)
	await act(async () => first.resolve(["/a/obsolete"]))
	expect({
		current: mount.textContent?.includes("/a/tree"),
		old: mount.textContent?.includes("/a/obsolete"),
	}).toEqual({ current: true, old: false })
})

test("newer retry owns loading and errors while previous results are ignored", async () => {
	listWorktrees.mockRejectedValueOnce(new Error("initial failure"))
	await render()
	const older = deferred<string[]>()
	const newer = deferred<string[]>()
	listWorktrees
		.mockImplementationOnce(() => older.promise)
		.mockImplementationOnce(() => newer.promise)
	const retry = button("Retry")
	await act(async () => {
		retry.click()
		retry.click()
	})
	await act(async () => older.resolve(["/a/stale"]))
	expect({
		stale: mount.textContent?.includes("/a/stale"),
		loading: !!mount.querySelector("svg.animate-spin"),
	}).toEqual({ stale: false, loading: true })
	await act(async () => newer.reject(new Error("latest failure")))
	expect(mount.querySelector('[role="alert"]')?.textContent).toContain("latest failure")
	await act(async () => button("Retry").click())
	expect({
		current: mount.textContent?.includes("/a/tree"),
		error: mount.querySelector('[role="alert"]')?.textContent,
	}).toEqual({ current: true, error: undefined })
})

test("metadata-only project updates do not issue extra worktree reads", async () => {
	await render()
	await act(async () => store.set(projects, [{ directory: "/a", name: "New display name" }]))
	expect(listWorktrees.mock.calls).toEqual([["/a"]])
})

test("unmount during mutation prevents follow-up reads", async () => {
	const mutation = deferred<void>()
	removeWorktree.mockImplementationOnce(() => mutation.promise)
	await render()
	await openRemove()
	await act(async () => button("Remove").click())
	await act(async () => root?.unmount())
	root = undefined
	await act(async () => mutation.resolve())
	expect({ reads: listWorktrees.mock.calls, writes: removeWorktree.mock.calls }).toEqual({
		reads: [["/a"]],
		writes: [["/a", "/a/tree"]],
	})
})

test("current failure stays in its confirmation and explicit retry succeeds", async () => {
	removeWorktree.mockRejectedValueOnce(new Error("Uncommitted changes"))
	await render()
	await openRemove()
	await act(async () => button("Remove").click())
	expect(document.querySelector('[role="alert"]')?.textContent).toBe("Uncommitted changes")
	expect(button("Remove").disabled).toBe(false)
	await act(async () => button("Remove").click())
	expect({
		dialog: !!document.querySelector('[role="dialog"]'),
		reads: listWorktrees.mock.calls,
		writes: removeWorktree.mock.calls,
	}).toEqual({
		dialog: false,
		reads: [["/a"], ["/a"]],
		writes: [
			["/a", "/a/tree"],
			["/a", "/a/tree"],
		],
	})
})
