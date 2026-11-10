import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { GitBranchInfo, GitStatusInfo } from "../../preload/api"

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

const branchInfo: GitBranchInfo = {
	state: "branch",
	detached: false,
	current: "main",
	local: ["main", "feature"],
	remote: [],
}
const cleanStatus: GitStatusInfo = {
	isClean: true,
	summary: "",
	staged: 0,
	modified: 0,
	untracked: 0,
	conflicted: 0,
}
const fetchGitBranches = mock(async (_directory: string) => branchInfo)
const fetchGitStatus = mock(async (_directory: string) => cleanStatus)
const gitCheckout = mock(async (_directory: string, _branch: string) => ({
	success: true,
}))
const gitStashAndCheckout = mock(async (_directory: string, _branch: string) => ({ success: true }))
mock.module("../services/backend", () => ({
	isElectron: true,
	fetchGitBranches,
	fetchGitStatus,
	gitCheckout,
	gitStashAndCheckout,
}))
const { createRoot } = await import("react-dom/client")
const { BranchPicker } = await import("./branch-picker")
const { useBranchPicker } = await import("./use-branch-picker")
const onBranchChanged = mock((_branch: string) => {})
let root: Root | undefined
let mount: HTMLElement

async function render(directory = "/project-a", activeSessionCount = 0) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root?.render(
			<BranchPicker
				directory={directory}
				currentBranch="main"
				onBranchChanged={onBranchChanged}
				activeSessionCount={activeSessionCount}
			/>,
		),
	)
}

function button(label: string): HTMLButtonElement {
	const match = [...document.querySelectorAll("button")].find(
		(element) => element.textContent === label,
	)
	if (!match) throw new Error(`Missing button ${label}: ${document.body.textContent}`)
	return match
}

async function click(label: string) {
	await act(async () => button(label).click())
}

function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}

afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	fetchGitBranches.mockReset().mockImplementation(async () => branchInfo)
	fetchGitStatus.mockReset().mockImplementation(async () => cleanStatus)
	gitCheckout.mockReset().mockImplementation(async () => ({ success: true }))
	gitStashAndCheckout.mockReset().mockImplementation(async () => ({ success: true }))
	onBranchChanged.mockClear()
})

test("changing projects discards a pending status check before checkout", async () => {
	const status = deferred<GitStatusInfo>()
	fetchGitStatus.mockImplementationOnce(() => status.promise)
	await render()
	await click("main")
	await click("feature")
	await render("/project-b")
	await act(async () => status.resolve(cleanStatus))
	expect(gitCheckout.mock.calls).toEqual([])
	expect(gitStashAndCheckout.mock.calls).toEqual([])
	expect(onBranchChanged.mock.calls).toEqual([])
})

test("a dirty confirmation does not move to another project", async () => {
	fetchGitStatus.mockImplementation(async () => ({
		...cleanStatus,
		isClean: false,
		modified: 1,
	}))
	await render()
	await click("main")
	await click("feature")
	expect(document.body.textContent).toContain("Uncommitted Changes")
	await render("/project-b")
	expect(Boolean(document.querySelector('[role="dialog"]'))).toBe(false)
	expect(gitStashAndCheckout.mock.calls).toEqual([])
})

test("a late branch list cannot label the newly selected project", async () => {
	const branches = deferred<GitBranchInfo>()
	fetchGitBranches.mockImplementationOnce(() => branches.promise)
	await render()
	await click("main")
	await render("/project-b")
	await act(async () => branches.resolve({ ...branchInfo, current: "old-project-branch" }))
	expect(document.body.textContent).not.toContain("old-project-branch")
	expect(button("main").disabled).toBe(false)
})

test("repeated selection starts one status check and one checkout", async () => {
	const status = deferred<GitStatusInfo>()
	fetchGitStatus.mockImplementation(() => status.promise)
	await render()
	await click("main")
	const item = button("feature")
	await act(async () => {
		item.click()
		item.click()
	})
	expect(fetchGitStatus.mock.calls).toEqual([["/project-a"]])
	await act(async () => status.resolve(cleanStatus))
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
	expect(onBranchChanged.mock.calls).toEqual([["feature"]])
})

test("a failed branch list is visible and can be retried", async () => {
	fetchGitBranches.mockRejectedValueOnce(new Error("Unable to read repository"))
	await render()
	await click("main")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain(
		"Unable to read repository",
	)
	await click("Retry")
	expect(button("feature")).toBeDefined()
	expect(fetchGitBranches.mock.calls).toEqual([["/project-a"], ["/project-a"]])
})

test("canceling dirty changes releases the picker without stashing", async () => {
	fetchGitStatus.mockResolvedValueOnce({
		...cleanStatus,
		isClean: false,
		modified: 1,
	})
	await render()
	await click("main")
	await click("feature")
	await click("Cancel")
	expect(gitStashAndCheckout.mock.calls).toEqual([])
	await click("main")
	await click("feature")
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
	expect(button("feature").disabled).toBe(false)
})

test("dirty changes and active sessions require both confirmations, even on repeated clicks", async () => {
	const checkout = deferred<{ success: boolean; stashed: boolean }>()
	gitStashAndCheckout.mockImplementation(() => checkout.promise)
	fetchGitStatus.mockResolvedValue({
		...cleanStatus,
		isClean: false,
		modified: 1,
	})
	await render("/project-a", 2)
	await click("main")
	await click("feature")
	const stash = button("Stash & Switch")
	await act(async () => {
		stash.click()
		stash.click()
	})
	expect(document.body.textContent).toContain("Active Sessions")
	expect(gitStashAndCheckout.mock.calls).toEqual([])
	const proceed = button("Switch Anyway")
	await act(async () => {
		proceed.click()
		proceed.click()
	})
	expect(gitStashAndCheckout.mock.calls).toEqual([["/project-a", "feature"]])
	expect(button("Switching...").disabled).toBe(true)
	await act(async () => checkout.resolve({ success: true, stashed: true }))
	expect(onBranchChanged.mock.calls).toEqual([["feature"]])
	expect(gitCheckout.mock.calls).toEqual([])
})

test("active-session warning cancellation does not check out and permits a later selection", async () => {
	await render("/project-a", 1)
	await click("main")
	await click("feature")
	await click("Cancel")
	expect(gitCheckout.mock.calls).toEqual([])
	await click("main")
	await click("feature")
	await click("Switch Anyway")
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
})

test("sessions that become active during status loading still receive a warning", async () => {
	const status = deferred<GitStatusInfo>()
	fetchGitStatus.mockImplementationOnce(() => status.promise)
	await render()
	await click("main")
	await click("feature")
	expect(button("Checking...").disabled).toBe(true)
	await render("/project-a", 1)
	await act(async () => status.resolve(cleanStatus))
	expect(document.body.textContent).toContain("Active Sessions")
	expect(gitCheckout.mock.calls).toEqual([])
	await click("Cancel")
})

for (const outcome of ["success", "failure"] as const) {
	test(`a dispatched checkout's late ${outcome} cannot update another project`, async () => {
		const checkout = deferred<{ success: boolean }>()
		gitCheckout.mockImplementationOnce(() => checkout.promise)
		await render()
		await click("main")
		await click("feature")
		await render("/project-b")
		await act(async () => {
			if (outcome === "success") checkout.resolve({ success: true })
			else checkout.reject(new Error("Old checkout failed"))
		})
		expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
		expect(onBranchChanged.mock.calls).toEqual([])
		expect(document.body.textContent).not.toContain("Old checkout failed")
		expect(button("main").disabled).toBe(false)
	})

	test(`closing and reopening ignores the obsolete branch-list ${outcome}`, async () => {
		const old = deferred<GitBranchInfo>()
		const current = deferred<GitBranchInfo>()
		fetchGitBranches.mockImplementationOnce(() => old.promise)
		fetchGitBranches.mockImplementationOnce(() => current.promise)
		await render()
		await click("main")
		await click("main")
		await click("main")
		await act(async () => {
			if (outcome === "success") old.resolve({ ...branchInfo, current: "obsolete" })
			else old.reject(new Error("Old list failed"))
		})
		expect(document.body.textContent).not.toContain("obsolete")
		expect(document.body.textContent).not.toContain("Old list failed")
		await act(async () => current.resolve(branchInfo))
		expect(Boolean(button("feature"))).toBe(true)
		expect(fetchGitBranches.mock.calls).toEqual([["/project-a"], ["/project-a"]])
	})
}

test("leaving and returning to the same project does not revive its previous selection", async () => {
	const status = deferred<GitStatusInfo>()
	fetchGitStatus.mockImplementationOnce(() => status.promise)
	await render()
	await click("main")
	await click("feature")
	await render("/project-b")
	await render()
	await act(async () => status.resolve(cleanStatus))
	expect(gitCheckout.mock.calls).toEqual([])
	expect(button("main").disabled).toBe(false)
})

test("unmount before status settles does not dispatch a checkout", async () => {
	const status = deferred<GitStatusInfo>()
	fetchGitStatus.mockImplementationOnce(() => status.promise)
	await render()
	await click("main")
	await click("feature")
	await act(async () => root?.unmount())
	root = undefined
	await act(async () => status.resolve(cleanStatus))
	expect(gitCheckout.mock.calls).toEqual([])
})

test("a status failure is visible and requires an explicit new selection", async () => {
	fetchGitStatus.mockRejectedValueOnce(new Error("Status unavailable"))
	await render()
	await click("main")
	await click("feature")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Status unavailable")
	expect(gitCheckout.mock.calls).toEqual([])
	await click("Dismiss")
	await click("main")
	await click("feature")
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
})

test("an unsuccessful checkout remains visible without replaying it", async () => {
	gitCheckout.mockResolvedValueOnce({ success: false })
	await render()
	await click("main")
	await click("feature")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Checkout failed")
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature"]])
	expect(onBranchChanged.mock.calls).toEqual([])
	expect(button("main").disabled).toBe(false)
})

test("local slash-containing branches and remote branches keep their checkout names", async () => {
	fetchGitBranches.mockResolvedValue({
		...branchInfo,
		local: ["main", "feature/local"],
		remote: ["origin/feature/remote"],
	})
	await render()
	await click("main")
	await click("feature/local")
	expect(gitCheckout.mock.calls).toEqual([["/project-a", "feature/local"]])
	await click("feature/local")
	await click("origin/feature/remote")
	expect(gitCheckout.mock.calls).toEqual([
		["/project-a", "feature/local"],
		["/project-a", "feature/remote"],
	])
})

for (const dialog of ["dirty", "active"] as const) {
	test(`Escape cancels the ${dialog} confirmation without changing Git state`, async () => {
		if (dialog === "dirty") {
			fetchGitStatus.mockResolvedValue({ ...cleanStatus, isClean: false, modified: 1 })
		}
		await render("/project-a", dialog === "active" ? 1 : 0)
		await click("main")
		await click("feature")
		await act(async () => {
			document.activeElement?.dispatchEvent(
				new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
			)
		})
		expect(Boolean(document.querySelector('[role="dialog"]'))).toBe(false)
		expect(gitCheckout.mock.calls).toEqual([])
		expect(gitStashAndCheckout.mock.calls).toEqual([])
		expect(button("main").disabled).toBe(false)
	})
}

for (const dialog of ["dirty", "active", "dirty-to-active"] as const) {
	test(`the exiting ${dialog} dialog cannot release checkout admission`, async () => {
		const pending = deferred<{ success: boolean; stashed: boolean }>()
		gitCheckout.mockImplementationOnce(() => pending.promise)
		gitStashAndCheckout.mockImplementationOnce(() => pending.promise)
		if (dialog !== "active") {
			fetchGitStatus.mockResolvedValue({ ...cleanStatus, isClean: false, modified: 1 })
		}
		await render("/project-a", dialog === "dirty" ? 0 : 1)
		await click("main")
		await click("feature")
		const popup = document.querySelector('[role="dialog"]') as HTMLElement
		const animation = deferred<void>()
		// Real Base UI retains this popup until its exit animation settles.
		popup.getAnimations = () => [{ finished: animation.promise }] as Animation[]
		await click(dialog === "active" ? "Switch Anyway" : "Stash & Switch")
		expect(popup.isConnected).toBe(true)
		expect(popup.hasAttribute("data-closed")).toBe(true)
		const cancel = [...popup.querySelectorAll("button")].find(
			(element) => element.textContent === "Cancel",
		)
		if (!cancel) throw new Error("Missing exiting dialog Cancel")
		expect(cancel.disabled).toBe(true)
		await act(async () => cancel.click())
		if (dialog === "dirty-to-active") {
			expect(document.body.textContent).toContain("Active Sessions")
			expect(gitStashAndCheckout.mock.calls).toEqual([])
			await click("Switch Anyway")
		}
		expect(button("Switching...").disabled).toBe(true)
		await act(async () => animation.resolve())
		await act(async () => button("Switching...").click())
		expect(Boolean(document.querySelector('[placeholder="Search branches..."]'))).toBe(false)
		expect(fetchGitStatus.mock.calls).toEqual([["/project-a"]])
		expect(gitCheckout.mock.calls).toEqual(dialog === "active" ? [["/project-a", "feature"]] : [])
		expect(gitStashAndCheckout.mock.calls).toEqual(
			dialog === "active" ? [] : [["/project-a", "feature"]],
		)
		await act(async () => pending.resolve({ success: true, stashed: true }))
		expect(onBranchChanged.mock.calls).toEqual([["feature"]])
		expect(button("feature").disabled).toBe(false)
	})
}

test("delayed confirmation handlers cannot act on a newer request or a dispatched checkout", async () => {
	let controls: ReturnType<typeof useBranchPicker> | undefined
	function Probe() {
		controls = useBranchPicker({
			directory: "/project-a",
			currentBranch: "main",
			activeSessionCount: 0,
			onBranchChanged,
		})
		return null
	}
	const pending = deferred<{ success: boolean; stashed: boolean }>()
	gitStashAndCheckout.mockImplementationOnce(() => pending.promise)
	fetchGitStatus.mockResolvedValue({ ...cleanStatus, isClean: false, modified: 1 })
	await render()
	await act(async () => root?.render(<Probe />))
	await act(async () => controls?.selectBranch("feature"))
	const oldConfirmation = controls
	await act(async () => controls?.cancelDirty())
	await act(async () => controls?.selectBranch("other"))
	await act(async () => {
		oldConfirmation?.cancelDirty()
		oldConfirmation?.stashAndSwitch()
	})
	expect(controls?.operation?.phase).toBe("dirty")
	expect(controls?.operation?.branch).toBe("other")
	expect(gitStashAndCheckout.mock.calls).toEqual([])
	const currentConfirmation = controls
	await act(async () => controls?.stashAndSwitch())
	await act(async () => {
		currentConfirmation?.cancelDirty()
		currentConfirmation?.stashAndSwitch()
	})
	expect(controls?.operation?.phase).toBe("switching")
	expect(gitStashAndCheckout.mock.calls).toEqual([["/project-a", "other"]])
	await act(async () => pending.resolve({ success: true, stashed: true }))
	expect(onBranchChanged.mock.calls).toEqual([["other"]])
	expect(controls?.operation).toBeNull()
})
