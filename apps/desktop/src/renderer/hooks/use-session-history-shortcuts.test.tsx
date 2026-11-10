import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import { useSessionHistoryShortcuts } from "./use-session-history-shortcuts"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let calls: string[] = []

function Transcript({
	id,
	active,
	canUndo = true,
	canRedo = true,
}: {
	id: string
	active: boolean
	canUndo?: boolean
	canRedo?: boolean
}) {
	useSessionHistoryShortcuts({
		isActive: active,
		canUndo,
		canRedo,
		onUndo: async () => {
			calls.push(`undo ${id}`)
			return undefined
		},
		onRedo: async () => {
			calls.push(`redo ${id}`)
		},
	})
	return <div aria-hidden={!active}>{id}</div>
}

async function render(active: string, canUndo = true, canRedo = true) {
	if (!root) {
		const mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root!.render(
			<>
				<Transcript id="A" active={active === "A"} canUndo={canUndo} canRedo={canRedo} />
				<Transcript id="B" active={active === "B"} canUndo={canUndo} canRedo={canRedo} />
			</>,
		),
	)
}

async function key(
	options: Pick<
		KeyboardEventInit,
		"key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "isComposing" | "keyCode"
	> = {},
	target: EventTarget = document.body,
	prevented = false,
) {
	const event = new dom.KeyboardEvent("keydown", {
		key: "z",
		ctrlKey: true,
		bubbles: true,
		cancelable: true,
		...options,
	})
	if (prevented) event.preventDefault()
	await act(async () => {
		target.dispatchEvent(event as unknown as Event)
	})
	return event
}

afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	calls = []
})

describe("session history shortcuts", () => {
	test("only the active mounted transcript handles undo and uppercase redo, also after switching", async () => {
		await render("A")
		expect((await key()).defaultPrevented).toBe(true)
		expect((await key({ key: "Z", shiftKey: true })).defaultPrevented).toBe(true)
		await render("B")
		await key({ ctrlKey: false, metaKey: true })
		await key({ key: "Z", shiftKey: true, ctrlKey: false, metaKey: true })
		expect(calls).toEqual(["undo A", "redo A", "undo B", "redo B"])
	})

	test("native input, textarea, and nested rich text history remain untouched", async () => {
		await render("A")
		const input = document.createElement("input")
		const textarea = document.createElement("textarea")
		const editable = document.createElement("div")
		editable.contentEditable = "true"
		const span = document.createElement("span")
		editable.append(span)
		document.body.append(input, textarea, editable)
		for (const target of [input, textarea, span]) {
			expect((await key({}, target)).defaultPrevented).toBe(false)
			expect((await key({ key: "Z", shiftKey: true }, target)).defaultPrevented).toBe(false)
		}
		expect(calls).toEqual([])
	})

	test("handled events, composition, unavailable history, and unmounted panels do nothing", async () => {
		await render("A")
		await key({}, document.body, true)
		for (const options of [
			{ isComposing: true },
			{ keyCode: 229 },
			{ altKey: true },
			{ ctrlKey: false },
		]) {
			expect((await key(options)).defaultPrevented).toBe(false)
		}
		await render("A", false, false)
		expect((await key()).defaultPrevented).toBe(false)
		expect((await key({ key: "Z", shiftKey: true })).defaultPrevented).toBe(false)
		await render("none")
		expect((await key()).defaultPrevented).toBe(false)
		await act(async () => root?.unmount())
		root = undefined
		expect((await key()).defaultPrevented).toBe(false)
		expect(calls).toEqual([])
	})
})
