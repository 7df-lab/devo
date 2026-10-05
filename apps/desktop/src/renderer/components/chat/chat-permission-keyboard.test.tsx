import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { Agent, PermissionRequest, PermissionResponse } from "../../lib/types"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	KeyboardEvent: dom.KeyboardEvent,
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { ChatPermissionFlow } = await import("./chat-permission")
const { handleSessionNavigationKeyDown } = await import("../root-layout-keyboard")
const agent = { id: "session-1" } as Agent
const permission: PermissionRequest = {
	id: "permission-1",
	requestId: "permission-1",
	sessionId: "session-1",
	permission: "read file",
	metadata: { availableScopes: ["once"] },
}
let root: Root | undefined
let mount: HTMLElement | undefined
let approvals: (PermissionResponse | undefined)[] = []
let denials: (string | undefined)[] = []

async function renderCard() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => {
		root!.render(
			<ChatPermissionFlow
				agent={agent}
				permission={permission}
				onApprove={async (_agent, _session, _permission, scope) => { approvals.push(scope) }}
				onDeny={async (_agent, _session, _permission, note) => { denials.push(note) }}
			/>,
		)
	})
	return mount.querySelector<HTMLElement>('section[aria-label="Tool permission request"]')!
}

async function key(target: EventTarget, name: string) {
	const event = new dom.KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true })
	await act(async () => { target.dispatchEvent(event as unknown as Event) })
	return event
}

afterEach(async () => {
	if (root) await act(async () => { root!.unmount() })
	root = undefined
	mount?.remove()
	mount = undefined
	document.body.replaceChildren()
	approvals = []
	denials = []
})

describe("permission card keyboard", () => {
	test("Enter elsewhere cannot approve Allow once; arrows and Escape elsewhere remain free", async () => {
		const card = await renderCard()
		const outside = document.createElement("button")
		document.body.append(outside)
		outside.focus()
		expect((await key(outside, "Enter")).defaultPrevented).toBe(false)
		expect((await key(outside, "ArrowDown")).defaultPrevented).toBe(false)
		expect((await key(outside, "Escape")).defaultPrevented).toBe(false)
		expect(approvals).toEqual([])
		expect(denials).toEqual([])
		expect(card.querySelector('[aria-selected="true"]')?.textContent).toContain("Allow once")
	})

	test("card arrows/Enter approve, Escape selects denial and note Enter denies", async () => {
		const card = await renderCard()
		card.focus()
		expect((await key(card, "Enter")).defaultPrevented).toBe(true)
		expect(approvals).toEqual(["once"])
		expect((await key(card, "Escape")).defaultPrevented).toBe(true)
		const note = card.querySelector<HTMLInputElement>("#permission-deny-note")!
		note.focus()
		await act(async () => {
			Object.getOwnPropertyDescriptor(dom.HTMLInputElement.prototype, "value")!.set!.call(note, "not now")
			note.dispatchEvent(new dom.Event("input", { bubbles: true }) as unknown as Event)
		})
		// The selected deny input stays inside the permission card.
		expect((await key(note, "Enter")).defaultPrevented).toBe(true)
		expect(denials).toEqual(["not now"])
		const outside = document.createElement("input")
		document.body.append(outside)
		outside.focus()
		expect((await key(outside, "Enter")).defaultPrevented).toBe(false)
		expect(denials).toEqual(["not now"])
	})

	test("card Escape wins over document capture session navigation", async () => {
		const card = await renderCard()
		const navigations: string[] = []
		const onCapture = (event: KeyboardEvent) => {
			handleSessionNavigationKeyDown(event, {
				agents: [{ id: "session-1", projectSlug: "project" }],
				sessionId: "session-1",
				navigate: (target) => navigations.push(target.to),
			})
		}
		document.addEventListener("keydown", onCapture, { capture: true })
		try {
			card.focus()
			expect((await key(card, "Escape")).defaultPrevented).toBe(true)
			expect(card.querySelector("#permission-deny-note")).not.toBeNull()
			expect(navigations).toEqual([])
		} finally {
			document.removeEventListener("keydown", onCapture, { capture: true })
		}
	})

	test("arrow navigation within the card selects denial without approving", async () => {
		const card = await renderCard()
		card.focus()
		expect((await key(card, "ArrowDown")).defaultPrevented).toBe(true)
		expect(card.querySelector("#permission-deny-note")).not.toBeNull()
		expect(approvals).toEqual([])
	})
})
