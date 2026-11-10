import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { GitCommitResult, GitDiffStat, GitPushResult } from "../../../preload/api"

const dom = new Window({ url: "http://localhost", width: 1188, height: 848 })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLTextAreaElement: dom.HTMLTextAreaElement,
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

const diffStat: GitDiffStat = {
	filesChanged: 1,
	insertions: 1,
	deletions: 0,
	files: [{ path: "example.ts", insertions: 1, deletions: 0 }],
}
const fetchDiffStat = mock(async (_directory: string): Promise<GitDiffStat> => diffStat)
const gitCommitAll = mock(
	async (_directory: string, _message: string): Promise<GitCommitResult> => ({
		success: true,
		commitHash: "test-commit",
	}),
)
const gitPush = mock(async (_directory: string): Promise<GitPushResult> => ({ success: true }))
mock.module("../../services/backend", () => ({ fetchDiffStat, gitCommitAll, gitPush }))
const { createRoot } = await import("react-dom/client")
const { CommitPushDialog } = await import("./commit-push-dialog")
const { useCommitPushDialog } = await import("./use-commit-push-dialog")
let onCommitted = mock(() => {})
let onOpenChange = mock((_open: boolean) => {})
let root: Root | undefined
let mount: HTMLElement

function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}

async function render(directory = "/project-a", open = true) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root?.render(
			<CommitPushDialog
				open={open}
				onOpenChange={onOpenChange}
				directory={directory}
				onCommitted={onCommitted}
			/>,
		),
	)
}

function button(label: string): HTMLButtonElement {
	const match = [...document.querySelectorAll("button")].find(
		(element) => element.textContent === label,
	)
	if (!match) throw new Error(`Missing ${label}: ${document.body.textContent}`)
	return match
}

async function click(label: string) {
	await act(async () => button(label).click())
}

beforeEach(() => {
	onCommitted = mock(() => {})
	onOpenChange = mock((_open: boolean) => {})
})
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	fetchDiffStat.mockReset().mockResolvedValue(diffStat)
	gitCommitAll.mockReset().mockResolvedValue({ success: true, commitHash: "test-commit" })
	gitPush.mockReset().mockResolvedValue({ success: true })
})

test("a failed push retries only the push without staging or committing again", async () => {
	gitPush.mockResolvedValueOnce({ success: false, error: "Network unavailable" })
	await render()
	await click("Commit & push")
	expect(document.body.textContent).toContain("Network unavailable")
	await click("Retry push")
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	expect(gitPush.mock.calls).toEqual([["/project-a"], ["/project-a"]])
	expect(onCommitted.mock.calls).toEqual([[]])
})

test("same-tick repeated submit admits only one commit", async () => {
	const commit = deferred<GitCommitResult>()
	gitCommitAll.mockImplementationOnce(() => commit.promise)
	await render()
	const submit = button("Commit & push")
	await act(async () => {
		submit.click()
		submit.click()
	})
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	await act(async () => commit.resolve({ success: true }))
	expect(gitPush.mock.calls).toEqual([["/project-a"]])
})

test("an obsolete scan cannot overwrite a reopened dialog", async () => {
	const scan = deferred<GitDiffStat>()
	fetchDiffStat.mockImplementationOnce(() => scan.promise)
	await render()
	await render("/project-a", false)
	await render()
	await act(async () => scan.resolve({ ...diffStat, filesChanged: 99 }))
	expect(document.body.textContent).toContain("1 file changed")
	expect(document.body.textContent).not.toContain("99 files changed")
})

test("a failed scan is visible and cannot authorize a blind commit", async () => {
	fetchDiffStat.mockRejectedValueOnce(new Error("Repository unavailable"))
	await render()
	expect(button("Commit & push").disabled).toBe(true)
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Repository unavailable")
	await click("Retry scan")
	expect(button("Commit & push").disabled).toBe(false)
	expect(gitCommitAll.mock.calls).toEqual([])
})

test("a previous success timer cannot close the reopened dialog", async () => {
	await render()
	await click("Commit & push")
	await render("/project-a", false)
	await render()
	await act(async () => new Promise((resolve) => setTimeout(resolve, 1300)))
	expect(onOpenChange.mock.calls).toEqual([])
})

test("navigating to another project before commit completion cannot start the old push", async () => {
	const commit = deferred<GitCommitResult>()
	gitCommitAll.mockImplementationOnce(() => commit.promise)
	await render()
	await click("Commit & push")
	await render("/project-b")
	await act(async () => commit.resolve({ success: true }))
	expect(gitPush.mock.calls).toEqual([])
	expect(onCommitted.mock.calls).toEqual([])
})

test("Cancel is available during the first scan and prevents a late result", async () => {
	const scan = deferred<GitDiffStat>()
	fetchDiffStat.mockImplementationOnce(() => scan.promise)
	await render()
	await click("Cancel")
	expect(onOpenChange.mock.calls).toEqual([[false]])
	await render("/project-a", false)
	await act(async () => scan.resolve(diffStat))
	expect(gitCommitAll.mock.calls).toEqual([])
})

test("a reported commit failure preserves the draft for an explicit retry", async () => {
	gitCommitAll.mockResolvedValueOnce({ success: false, error: "Commit hook failed" })
	await render()
	const message = document.querySelector("textarea") as HTMLTextAreaElement
	await act(async () => {
		Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(
			message,
			"  Fix navigation  ",
		)
		message.dispatchEvent(new Event("input", { bubbles: true }))
	})
	await click("Commit & push")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Commit hook failed")
	expect(message.disabled).toBe(false)
	expect(message.value).toBe("  Fix navigation  ")
	expect(gitPush.mock.calls).toEqual([])
	await click("Commit & push")
	expect(gitCommitAll.mock.calls).toEqual([
		["/project-a", "Fix navigation"],
		["/project-a", "Fix navigation"],
	])
	expect(gitPush.mock.calls).toEqual([["/project-a"]])
})

test("an uncertain commit result never enables another commit or a push", async () => {
	gitCommitAll.mockRejectedValueOnce(new Error("IPC disconnected"))
	await render()
	await click("Commit & push")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Check Git history")
	expect(button("Commit & push").disabled).toBe(true)
	await click("Commit & push")
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	expect(gitPush.mock.calls).toEqual([])
	expect(onCommitted.mock.calls).toEqual([])
})

test("an uncertain push result retains the local commit and allows only explicit push retry", async () => {
	gitPush.mockRejectedValueOnce(new Error("IPC disconnected"))
	await render()
	await click("Commit & push")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("Check the remote")
	expect((document.querySelector("textarea") as HTMLTextAreaElement).disabled).toBe(true)
	await click("Retry push")
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	expect(gitPush.mock.calls).toEqual([["/project-a"], ["/project-a"]])
	expect(onCommitted.mock.calls).toEqual([[]])
})

test("repeated Retry push clicks start one request and cannot close pending work", async () => {
	const push = deferred<GitPushResult>()
	gitPush.mockResolvedValueOnce({ success: false, error: "Network unavailable" })
	gitPush.mockImplementationOnce(() => push.promise)
	await render()
	await click("Commit & push")
	const retry = button("Retry push")
	await act(async () => {
		retry.click()
		retry.click()
	})
	expect(button("Pushing...").disabled).toBe(true)
	expect(button("Cancel").disabled).toBe(true)
	await click("Cancel")
	await act(async () =>
		document.activeElement?.dispatchEvent(
			new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
		),
	)
	expect(onOpenChange.mock.calls).toEqual([])
	expect(gitPush.mock.calls).toEqual([["/project-a"], ["/project-a"]])
	await act(async () => push.resolve({ success: true }))
	expect(button("Done").disabled).toBe(false)
	expect(onCommitted.mock.calls).toEqual([[]])
})

test("empty scans keep commit disabled", async () => {
	fetchDiffStat.mockResolvedValue({ ...diffStat, filesChanged: 0, files: [] })
	await render()
	expect(document.body.textContent).toContain("No changes detected")
	expect(button("Commit & push").disabled).toBe(true)
	await click("Commit & push")
	expect(gitCommitAll.mock.calls).toEqual([])
})

test("success replaces submit with Done and cannot replay the old submit handler", async () => {
	await render()
	const submit = button("Commit & push")
	await act(async () => {
		submit.click()
		submit.click()
	})
	expect(button("Done").disabled).toBe(false)
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	await click("Done")
	expect(onOpenChange.mock.calls).toEqual([[false]])
})

for (const outcome of ["success", "failure"] as const) {
	test(`a stale push ${outcome} cannot affect A after A → B → A`, async () => {
		const push = deferred<GitPushResult>()
		gitPush.mockImplementationOnce(() => push.promise)
		await render()
		await click("Commit & push")
		await render("/project-b")
		await render()
		await act(async () => {
			if (outcome === "success") push.resolve({ success: true })
			else push.reject(new Error("Obsolete push failure"))
		})
		expect(document.body.textContent).not.toContain("Committed and pushed")
		expect(document.body.textContent).not.toContain("Obsolete push failure")
		expect(button("Commit & push").disabled).toBe(false)
		expect(onCommitted.mock.calls).toEqual([[]])
	})
}

test("unmount during commit suppresses refresh and follow-up push", async () => {
	const commit = deferred<GitCommitResult>()
	gitCommitAll.mockImplementationOnce(() => commit.promise)
	await render()
	await click("Commit & push")
	await act(async () => root?.unmount())
	root = undefined
	await act(async () => commit.resolve({ success: true }))
	expect(gitPush.mock.calls).toEqual([])
	expect(onCommitted.mock.calls).toEqual([])
})

test("a previous operation cannot unlock a newer pending commit", async () => {
	const old = deferred<GitCommitResult>()
	const current = deferred<GitCommitResult>()
	gitCommitAll.mockImplementationOnce(() => old.promise)
	gitCommitAll.mockImplementationOnce(() => current.promise)
	await render()
	await click("Commit & push")
	await render("/project-a", false)
	await render()
	await click("Commit & push")
	await act(async () => old.resolve({ success: true }))
	expect(button("Committing...").disabled).toBe(true)
	expect(gitPush.mock.calls).toEqual([])
	await act(async () => current.resolve({ success: true }))
	expect(gitPush.mock.calls).toEqual([["/project-a"]])
	expect(onCommitted.mock.calls).toEqual([[]])
})

test("stale submit and dismissal handlers cannot act on a reopened visit", async () => {
	let controls: ReturnType<typeof useCommitPushDialog> | undefined
	function Probe({ open }: { open: boolean }) {
		controls = useCommitPushDialog({ open, directory: "/project-a", onOpenChange, onCommitted })
		return null
	}
	await render()
	await act(async () => root?.render(<Probe open />))
	const old = controls
	await act(async () => root?.render(<Probe open={false} />))
	await act(async () => root?.render(<Probe open />))
	await act(async () => {
		old?.changeOpen(false)
		void old?.execute()
		old?.retryScan()
	})
	expect(onOpenChange.mock.calls).toEqual([])
	expect(gitCommitAll.mock.calls).toEqual([])
	await act(async () => controls?.execute())
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
})

test("closing controls remain inert throughout the dialog exit animation", async () => {
	await render()
	const popup = document.querySelector('[role="dialog"]') as HTMLElement
	const animation = deferred<Animation>()
	const runningAnimation = { finished: animation.promise } as Animation
	popup.getAnimations = () => [runningAnimation]
	const submit = button("Commit & push")
	const cancel = button("Cancel")
	await click("Cancel")
	await render("/project-a", false)
	expect(popup.isConnected).toBe(true)
	expect(popup.hasAttribute("data-closed")).toBe(true)
	expect(submit.disabled).toBe(true)
	expect(cancel.disabled).toBe(true)
	await act(async () => {
		submit.click()
		cancel.click()
	})
	expect(gitCommitAll.mock.calls).toEqual([])
	expect(onOpenChange.mock.calls).toEqual([[false]])
	await act(async () => animation.resolve(runningAnimation))
})

test("reopening after a dismissed push failure starts a fresh scan, not an implicit retry", async () => {
	gitPush.mockResolvedValueOnce({ success: false, error: "Network unavailable" })
	await render()
	await click("Commit & push")
	await click("Cancel")
	await render("/project-a", false)
	fetchDiffStat.mockResolvedValue({ ...diffStat, filesChanged: 0, files: [] })
	await render()
	expect(document.body.textContent).toContain("No changes detected")
	expect(document.body.textContent).not.toContain("Retry push")
	expect(gitCommitAll.mock.calls).toEqual([["/project-a", "Update 1 file"]])
	expect(gitPush.mock.calls).toEqual([["/project-a"]])
})
