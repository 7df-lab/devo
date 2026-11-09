import { afterEach, beforeEach, describe, expect, jest, mock, test } from "bun:test"
import type { NativeSession, SessionRosterChange, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { getDefaultStore } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import type { Agent } from "../lib/types"

const listeners = new Set<(change: SessionRosterChange) => void>()
const observeRoster = (listener: (change: SessionRosterChange) => void) => {
	listeners.add(listener)
	return () => {
		listeners.delete(listener)
	}
}
function emit(change: SessionRosterChange) {
	for (const listener of listeners) listener(change)
}
beforeEach(() => {
	getDefaultStore().set(serverConnectedAtom, true)
})

const dom = new Window({ url: "http://localhost" })
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
let page: SessionRosterPage = { data: [], nextCursor: null }
let calls = 0
const navigations: unknown[] = []
const aborts: Array<{ sessionId: string; serverUrl: string | null }> = []
let interrupt: () => Promise<void> = async () => {}
function deferred() {
	let resolve!: () => void
	let reject!: (error: Error) => void
	const promise = new Promise<void>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => {
		const serverUrl = getDefaultStore().get(serverUrlAtom)
		return {
			session: {
				observeRoster,
				abort: async ({ sessionId }: { sessionId: string }) => {
					aborts.push({ sessionId, serverUrl })
					await interrupt()
				},
				roster: async () => {
					calls++
					return page
				},
			},
		}
	},
}))
mock.module("@tanstack/react-router", () => ({
	useNavigate: () => (route: unknown) => {
		navigations.push(route)
	},
}))
const { SubagentsButton, subagentStatus } = await import("./subagents-button")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let mount: HTMLElement
const agent = {
	sessionId: "parent",
	directory: "/repo",
	projectSlug: "repo-1",
} as Agent
const child = {
	id: "child",
	title: "Check runtime",
	parent: { kind: "agent", sessionId: "parent" },
	ephemeral: false,
	flags: [],
	status: "idle",
	queuedCount: 0,
} as unknown as NativeSession

async function render() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<SubagentsButton agent={agent} />))
}
function button(label: string) {
	const element = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)
	if (!element) throw new Error(`Missing ${label}`)
	return element
}
async function click(element: HTMLElement) {
	await act(async () => {
		element.click()
		await Bun.sleep(30)
	})
}
afterEach(async () => {
	await act(async () => {
		root?.unmount()
		await Bun.sleep(30)
	})
	root = undefined
	mount?.remove()
	jest.useRealTimers()
	expect(listeners.size).toBe(0)
	getDefaultStore().set(serverUrlAtom, null)
	page = { data: [], nextCursor: null }
	calls = 0
	navigations.length = 0
	aborts.length = 0
	interrupt = async () => {}
})

describe("subagent labels", () => {
	for (const [patch, label] of [
		[{}, "Idle"],
		[{ status: "active" }, "Working"],
		[{ queuedCount: 2 }, "Queued"],
		[{ status: "active", flags: ["waitingApproval"] }, "Needs approval"],
		[{ flags: ["waitingUserInput"] }, "Needs input"],
		[{ flags: ["compacting"] }, "Compacting"],
		[{ flags: ["updatingGoal"] }, "Updating goal"],
	] as const) {
		test(label, () => expect(subagentStatus({ ...child, ...patch } as NativeSession)).toBe(label))
	}
})

describe("subagent popover interactions", () => {
	test("unscoped runtime results show an actionable error without child navigation", async () => {
		page = { data: [{ ...child, parent: undefined }], nextCursor: "200" }
		await render()
		await click(button("Subagents"))
		expect(document.querySelector("[role=alert]")?.textContent).toContain("Update the runtime")
		expect(document.querySelectorAll("ul[aria-label='Subagent sessions'] li")).toHaveLength(0)
		expect(calls).toBe(1)
		expect(navigations).toEqual([])
	})

	test("opens on demand, shows child state and navigates using the real session ID", async () => {
		page = {
			data: [{ ...child, flags: ["waitingApproval"] }],
			nextCursor: null,
		}
		await render()
		expect(calls).toBe(0)
		await click(button("Subagents"))
		expect(calls).toBe(1)
		expect(document.body.textContent).toContain("Needs approval")
		expect(document.body.textContent).toContain("Idle does not confirm completion")
		const row = document.querySelector<HTMLButtonElement>(
			"ul[aria-label='Subagent sessions'] button",
		)
		if (!row) throw new Error("Missing child row")
		await click(row)
		expect(navigations).toEqual([
			{
				to: "/project/$projectSlug/session/$sessionId",
				params: { projectSlug: "repo-1", sessionId: "child" },
			},
		])
		expect(button("Subagents").getAttribute("aria-expanded")).toBe("false")
	})
	test("close and Escape dismiss without navigation and restore trigger focus", async () => {
		await render()
		button("Subagents").focus()
		await click(button("Subagents"))
		await click(button("Close subagents"))
		expect(button("Subagents").getAttribute("aria-expanded")).toBe("false")
		expect(document.activeElement).toBe(button("Subagents"))
		await click(button("Subagents"))
		await act(async () => {
			document.activeElement?.dispatchEvent(
				new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
			)
			await Bun.sleep(30)
		})
		expect(button("Subagents").getAttribute("aria-expanded")).toBe("false")
		expect(navigations).toEqual([])
	})
	test("refresh updates empty state and does not start model work", async () => {
		await render()
		await click(button("Subagents"))
		expect(document.body.textContent).toContain("No saved subagents")
		page = { data: [child], nextCursor: null }
		await click(button("Refresh subagents"))
		expect(document.body.textContent).toContain("Check runtime")
		expect(calls).toBe(2)
		expect(navigations).toEqual([])
	})
})

test("open popover follows live status, creation and deletion without clicking Refresh", async () => {
	page = { data: [child], nextCursor: null }
	await render()
	await click(button("Subagents"))
	await act(async () => {
		emit({
			type: "patch",
			sessionId: child.id,
			patch: { status: "active", flags: ["waitingUserInput"] },
		})
	})
	expect(document.body.textContent).toContain("Needs input")
	expect(document.body.textContent).toContain("Updates automatically while open")
	await act(async () => {
		emit({ type: "upsert", session: { ...child, id: "new", title: "New child" } })
		emit({ type: "delete", sessionIds: [child.id] })
	})
	expect(document.body.textContent).toContain("New child")
	expect(document.body.textContent).not.toContain("Check runtime")
	expect(calls).toBe(1)
	await click(button("Close subagents"))
	expect(listeners.size).toBe(0)
})

describe("subagent stop controls", () => {
	test("interrupts the chosen active child without navigation or pretending it is idle", async () => {
		page = {
			data: [
				{ ...child, status: "active", queuedCount: 2 },
				{ ...child, id: "other", title: "Other child", status: "active" },
			],
			nextCursor: null,
		}
		await render()
		await click(button("Subagents"))
		await click(button("Stop Check runtime"))
		expect(aborts).toEqual([{ sessionId: "child", serverUrl: null }])
		expect(navigations).toEqual([])
		expect(button("Subagents").getAttribute("aria-expanded")).toBe("true")
		expect(document.body.textContent).toContain("Working")
		expect(document.body.textContent).toContain("Queued messages may still run")
		expect(calls).toBe(2)
		await act(async () => emit({ type: "patch", sessionId: "child", patch: { status: "idle" } }))
		expect(document.querySelector('button[aria-label="Stop Check runtime"]')).toBeNull()
		expect(document.body.textContent).toContain("Queued")
		expect(button("Stop Other child").disabled).toBe(false)
	})

	test("idle, queued-only and archived children have no stop control", async () => {
		page = {
			data: [
				child,
				{ ...child, id: "queued", queuedCount: 1 },
				{ ...child, id: "archived", status: "active", archived: true },
			],
			nextCursor: null,
		}
		await render()
		await click(button("Subagents"))
		expect(document.querySelectorAll('button[aria-label^="Stop "]')).toHaveLength(0)
		expect(aborts).toEqual([])
	})

	test("repeated clicks and close/reopen do not duplicate a pending interrupt", async () => {
		const pending = deferred()
		interrupt = () => pending.promise
		page = { data: [{ ...child, status: "active" }], nextCursor: null }
		await render()
		await click(button("Subagents"))
		await act(async () => {
			button("Stop Check runtime").click()
			button("Stop Check runtime").click()
		})
		expect(button("Stop Check runtime").disabled).toBe(true)
		await click(button("Close subagents"))
		await click(button("Subagents"))
		expect(button("Stop Check runtime").disabled).toBe(true)
		expect(document.body.textContent).toContain("Requesting stop")
		await click(button("Stop Check runtime"))
		expect(aborts).toHaveLength(1)
		await act(async () => pending.resolve())
		expect(button("Stop Check runtime").disabled).toBe(false)
		expect(document.body.textContent).not.toContain("Requesting stop")
	})

	test("failed or uncertain interrupts surface an error and are never automatically replayed", async () => {
		interrupt = async () => {
			throw new Error("Connection lost before acknowledgement")
		}
		page = { data: [{ ...child, status: "active" }], nextCursor: null }
		await render()
		await click(button("Subagents"))
		await click(button("Stop Check runtime"))
		expect(document.querySelector('[role="alert"]')?.textContent).toContain(
			"Check the child's state before retrying",
		)
		expect(document.body.textContent).toContain("Connection lost before acknowledgement")
		await click(button("Refresh subagents"))
		await click(button("Close subagents"))
		await click(button("Subagents"))
		expect(aborts).toHaveLength(1)
		interrupt = async () => {}
		await click(button("Stop Check runtime"))
		expect(aborts).toHaveLength(2)
		expect(document.querySelector('[role="alert"]')).toBeNull()
	})

	test("disconnect disables controls and reconnect does not replay an uncertain stop", async () => {
		const pending = deferred()
		interrupt = () => pending.promise
		page = { data: [{ ...child, status: "active" }], nextCursor: null }
		await render()
		await click(button("Subagents"))
		await click(button("Stop Check runtime"))
		await act(async () => emit({ type: "disconnected" }))
		await act(async () => pending.reject(new Error("Disconnected")))
		expect(button("Stop Check runtime").disabled).toBe(true)
		await click(button("Stop Check runtime"))
		await act(async () => emit({ type: "resync" }))
		expect(aborts).toHaveLength(1)
		expect(button("Stop Check runtime").disabled).toBe(false)
		expect(document.body.textContent).toContain("Stop was not confirmed")
	})

	test("late results stay with the original parent and do not affect another roster", async () => {
		const pending = deferred()
		interrupt = () => pending.promise
		page = { data: [{ ...child, status: "active" }], nextCursor: null }
		await render()
		await click(button("Subagents"))
		await click(button("Stop Check runtime"))
		page = {
			data: [
				{ ...child, status: "active", parent: { kind: "agent", sessionId: "different-parent" } },
			],
			nextCursor: null,
		}
		await act(async () =>
			root?.render(<SubagentsButton agent={{ ...agent, sessionId: "different-parent" }} />),
		)
		expect(button("Stop Check runtime").disabled).toBe(false)
		await act(async () => pending.reject(new Error("Old parent request failed")))
		expect(document.querySelector('[role="alert"]')).toBeNull()
		expect(document.body.textContent).not.toContain("Requesting stop")
		expect(aborts).toEqual([{ sessionId: "child", serverUrl: null }])
	})

	test("server switching uses the selected server and isolates late control results", async () => {
		const pending = deferred()
		interrupt = () => pending.promise
		page = { data: [{ ...child, status: "active" }], nextCursor: null }
		await render()
		await click(button("Subagents"))
		await click(button("Stop Check runtime"))
		await act(async () => getDefaultStore().set(serverUrlAtom, "http://second-runtime"))
		expect(button("Stop Check runtime").disabled).toBe(false)
		await act(async () => pending.reject(new Error("Old runtime disconnected")))
		expect(document.querySelector('[role="alert"]')).toBeNull()
		interrupt = async () => {}
		await click(button("Stop Check runtime"))
		expect(aborts).toEqual([
			{ sessionId: "child", serverUrl: null },
			{ sessionId: "child", serverUrl: "http://second-runtime" },
		])
	})
})
