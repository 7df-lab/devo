import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import { renderToStaticMarkup } from "react-dom/server"
import type { ChatMessageEntry } from "../../hooks/use-session-chat"

const dom = new Window({ url: "http://localhost" })
dom.document.write("<!doctype html><html><head></head><body></body></html>")
dom.document.close()
// Happy DOM does not implement compatMode; this document has a standards doctype.
Object.defineProperty(dom.document, "compatMode", { value: "CSS1Compat" })
Object.assign(globalThis, {
	window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement,
	MutationObserver: dom.MutationObserver, ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { Conversation } = await import("@devo/ui/components/ai-elements/conversation")
const { TurnActivity } = await import("./turn-activity")
let root: Root | undefined

afterEach(async () => {
	if (root) await act(async () => root!.unmount())
	root = undefined
	document.body.replaceChildren()
})

const entries: ChatMessageEntry[] = [
	{ id: "thought", item: { type: "reasoning", text: "Private thought details" } },
	{ id: "python", item: { type: "toolCall", callId: "c1", toolName: "ipython", input: { code: "print(42)" } } },
	{ id: "output", item: { type: "toolResult", callId: "c1", output: "42", isError: false } },
].map(({ id, item }) => ({ info: {
	id, sessionId: "s1", turnId: "t1", seq: 1, revision: 1, state: "completed",
	createdAt: "2026-10-09T02:00:00Z", updatedAt: "2026-10-09T02:00:01Z", item,
} }))

describe("separate activity disclosures", () => {
	test("Worked hides Thought and Python until expanded", () => {
		const html = renderToStaticMarkup(<Conversation><TurnActivity entries={entries} working={false} /></Conversation>)
		const controls = [...html.matchAll(/<button\b[^>]*>/g)].map(([tag]) => ({
			label: tag.match(/aria-label="([^"]+)"/)?.[1],
			expanded: tag.match(/aria-expanded="([^"]+)"/)?.[1],
		}))
		expect({
			controls,
			elapsedTime: html.includes("Worked for 1s"),
			thoughtVisible: html.includes("Private thought details"),
			pythonVisible: html.includes("print(42)"),
		}).toEqual({
			controls: [{ label: "Worked for 1s: activity details", expanded: "false" }],
			elapsedTime: true,
			thoughtVisible: false,
			pythonVisible: false,
		})
	})
	test("inner cells expand independently and Worked collapses both", async () => {
		const mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
		await act(async () => root!.render(<Conversation><TurnActivity entries={entries} working={false} /></Conversation>))
		const outer = mount.querySelector<HTMLButtonElement>('button[aria-label="Worked for 1s: activity details"]')!
		await act(async () => outer.click())
		const thought = mount.querySelector<HTMLButtonElement>('button[aria-label="Thought: activity details"]')!
		const python = mount.querySelector<HTMLButtonElement>('button[aria-label="Python: activity details"]')!
		expect([thought.getAttribute("aria-expanded"), python.getAttribute("aria-expanded")]).toEqual(["false", "false"])
		await act(async () => thought.click())
		expect({
			states: [thought.getAttribute("aria-expanded"), python.getAttribute("aria-expanded")],
			thoughtVisible: mount.textContent!.includes("Private thought details"),
			pythonVisible: mount.textContent!.includes("print(42)"),
		}).toEqual({ states: ["true", "false"], thoughtVisible: true, pythonVisible: false })
		await act(async () => python.click())
		await act(async () => thought.click())
		expect({
			states: [thought.getAttribute("aria-expanded"), python.getAttribute("aria-expanded")],
			thoughtVisible: mount.textContent!.includes("Private thought details"),
			pythonVisible: mount.textContent!.includes("42"),
		}).toEqual({ states: ["false", "true"], thoughtVisible: false, pythonVisible: true })
		await act(async () => outer.click())
		expect({
			expanded: outer.getAttribute("aria-expanded"),
			innerControls: mount.querySelectorAll(
				'button[aria-label="Thought: activity details"],button[aria-label="Python: activity details"]',
			).length,
		}).toEqual({ expanded: "false", innerControls: 0 })
	})
	test("finishing a turn collapses an expanded working group", async () => {
		const mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
		await act(async () => root!.render(<Conversation><TurnActivity entries={entries} working /></Conversation>))
		await act(async () => mount.querySelector<HTMLButtonElement>('button[aria-label="Working…: activity details"]')!.click())
		await act(async () => mount.querySelector<HTMLButtonElement>('button[aria-label="Python: activity details"]')!.click())
		expect(mount.textContent!.includes("print(42)")).toBe(true)
		await act(async () => root!.render(<Conversation><TurnActivity entries={entries} working={false} /></Conversation>))
		expect({
			expanded: mount.querySelector('button[aria-label="Worked for 1s: activity details"]')!.getAttribute("aria-expanded"),
			innerControls: mount.querySelectorAll('button[aria-label="Thought: activity details"],button[aria-label="Python: activity details"]').length,
			codeVisible: mount.textContent!.includes("print(42)"),
		}).toEqual({ expanded: "false", innerControls: 0, codeVisible: false })
	})
	test("working before any items is a status caption without a dead expand control", () => {
		const html = renderToStaticMarkup(<Conversation><TurnActivity entries={[]} working /></Conversation>)
		expect({ working: html.includes("Working…"), expandable: html.includes("<button") }).toEqual({ working: true, expandable: false })
	})
})
