import { afterEach, describe, expect, mock, test } from "bun:test"
import type { NativeSession, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { Agent } from "../lib/types"

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
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => ({
		session: {
			roster: async () => {
				calls++
				return page
			},
		},
	}),
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
	page = { data: [], nextCursor: null }
	calls = 0
	navigations.length = 0
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
