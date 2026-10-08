import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/dialog", () => ({
	Dialog: ({ children, open }: { children?: ReactNode; open: boolean }) => open ? <div>{children}</div> : null,
	DialogContent: wrap, DialogDescription: wrap, DialogFooter: wrap, DialogHeader: wrap, DialogTitle: wrap }))
mock.module("../../hooks/use-agents", () => ({ useProjectList: () => [{ directory: "C:/QA/project", name: "Project" }] }))
const listWorktrees = mock(async () => ["C:\\QA\\worktrees\\settings-qa"])
const removeWorktree = mock(async (_project: string, _directory: string) => { throw new Error("Worktree has uncommitted changes") })
const resetWorktree = mock(async (_project: string, _directory: string) => {})
mock.module("../../services/worktree-service", () => ({ listWorktrees, removeWorktree, resetWorktree }))
const { createRoot } = await import("react-dom/client")
const { WorktreeSettings } = await import("./worktree-settings")
let root: Root | undefined
let mount: HTMLElement
async function renderSettings() {
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
	await act(async () => root?.render(<WorktreeSettings />))
}
afterEach(async () => { await act(async () => root?.unmount()); mount?.remove(); listWorktrees.mockClear(); removeWorktree.mockClear(); resetWorktree.mockClear() })
test("Windows worktree names use the last path segment", async () => {
	await renderSettings()
	expect(mount.querySelector("span.truncate")?.textContent).toBe("settings-qa")
})
test("removal requires confirmation and failures remain visible", async () => {
	await renderSettings()
	await act(async () => mount.querySelector<HTMLButtonElement>('[title="Remove worktree"]')?.click())
	expect(removeWorktree.mock.calls).toEqual([])
	const button = [...mount.querySelectorAll("button")].find((button) => button.textContent === "Remove")
	await act(async () => button?.click())
	expect(mount.querySelector('[role="alert"]')?.textContent).toBe("Worktree has uncommitted changes")
	expect(removeWorktree.mock.calls).toEqual([["C:/QA/project", "C:\\QA\\worktrees\\settings-qa"]])
})
test("listing errors expose Retry instead of an apparently empty repository", async () => {
	listWorktrees.mockImplementationOnce(async () => { throw new Error("Git unavailable") })
	await renderSettings()
	expect(mount.querySelector('[role="alert"]')?.textContent).toContain("Git unavailable")
	const retry = [...mount.querySelectorAll("button")].find((button) => button.textContent === "Retry")
	await act(async () => retry?.click())
	expect({ alert: mount.querySelector('[role="alert"]'), name: mount.querySelector("span.truncate")?.textContent })
		.toEqual({ alert: null, name: "settings-qa" })
})
