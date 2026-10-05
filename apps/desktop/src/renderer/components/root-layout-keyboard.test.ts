import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import {
	handleSessionNavigationKeyDown,
	isSessionNavigationBlocked,
	shouldToggleCommandPalette,
} from "./root-layout-keyboard"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	KeyboardEvent: dom.KeyboardEvent,
})

const agents = [
	{ id: "one", projectSlug: "project" },
	{ id: "two", projectSlug: "project" },
]
const navigations: string[] = []
const listener = (event: KeyboardEvent) =>
	handleSessionNavigationKeyDown(event, {
		agents,
		sessionId: "one",
		navigate: (target) => navigations.push(target.params?.sessionId ?? target.to),
	})
document.addEventListener("keydown", listener, { capture: true })

function key(target: EventTarget, name: string, options: { ctrlKey?: boolean } = {}): KeyboardEvent {
	const event = new dom.KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...options })
	target.dispatchEvent(event as unknown as Event)
	return event as unknown as KeyboardEvent
}

afterEach(() => {
	document.body.replaceChildren()
	navigations.length = 0
})

describe("root session navigation keyboard capture", () => {
	test("Escape/j/k work in the unblocked session area", () => {
		const area = document.createElement("section")
		document.body.append(area)
		expect(key(area, "j").defaultPrevented).toBe(true)
		expect(key(area, "k").defaultPrevented).toBe(true)
		expect(key(area, "Escape").defaultPrevented).toBe(true)
		expect(navigations).toEqual(["two", "two", "/"])
	})

	test("dialog Escape does not navigate or swallow the dialog's key", () => {
		const dialog = document.createElement("div")
		dialog.setAttribute("role", "dialog")
		dialog.setAttribute("data-open", "")
		document.body.append(dialog)
		const event = key(dialog, "Escape")
		expect(event.defaultPrevented).toBe(false)
		expect(navigations).toEqual([])
		// Even if the modal's focus leaves its portal temporarily, the modal blocks navigation.
		expect(key(document.body, "j").defaultPrevented).toBe(false)
		expect(navigations).toEqual([])
	})

	test("Cmd/Ctrl shortcuts avoid dialogs and contenteditable, but work in the session area", () => {
		const triggered: string[] = []
		const onShortcut = (event: KeyboardEvent) => {
			if (isSessionNavigationBlocked(event)) return
			if (event.ctrlKey && (event.key === "n" || event.key === "k")) {
				event.preventDefault()
				triggered.push(event.key)
			}
		}
		document.addEventListener("keydown", onShortcut, { capture: true })
		try {
			const dialog = document.createElement("div")
			dialog.setAttribute("role", "dialog")
			dialog.setAttribute("data-open", "")
			document.body.append(dialog)
			const dialogButton = document.createElement("button")
			dialog.append(dialogButton)
			for (const name of ["n", "k"]) {
				expect(key(dialogButton, name, { ctrlKey: true }).defaultPrevented).toBe(false)
			}
			// The portal also blocks shortcuts if focus briefly moves into the session area.
			expect(key(document.body, "n", { ctrlKey: true }).defaultPrevented).toBe(false)
			dialog.remove()
			const editor = document.createElement("div")
			editor.setAttribute("contenteditable", "plaintext-only")
			document.body.append(editor)
			expect(key(editor, "k", { ctrlKey: true }).defaultPrevented).toBe(false)
			expect(key(editor, "Escape").defaultPrevented).toBe(false)
			editor.remove()
			expect(key(document.body, "n", { ctrlKey: true }).defaultPrevented).toBe(true)
			expect(key(document.body, "k", { ctrlKey: true }).defaultPrevented).toBe(true)
			expect(triggered).toEqual(["n", "k"])
			expect(navigations).toEqual([])
		} finally {
			document.removeEventListener("keydown", onShortcut, { capture: true })
		}
	})

	test("palette Cmd/Ctrl+K ignores other dialogs, opens from input, and toggles its own dialog", () => {
		let open = false
		const toggles: boolean[] = []
		const onPaletteKeyDown = (event: KeyboardEvent) => {
			if (!shouldToggleCommandPalette(event, open)) return
			event.preventDefault()
			open = !open
			toggles.push(open)
		}
		document.addEventListener("keydown", onPaletteKeyDown)
		try {
			const composer = document.createElement("input")
			document.body.append(composer)
			const otherDialog = document.createElement("div")
			otherDialog.setAttribute("role", "dialog")
			otherDialog.setAttribute("data-open", "")
			document.body.append(otherDialog)
			expect(key(otherDialog, "k", { ctrlKey: true }).defaultPrevented).toBe(false)
			expect(toggles).toEqual([])
			otherDialog.remove()
			expect(key(composer, "k", { ctrlKey: true }).defaultPrevented).toBe(true)
			expect(toggles).toEqual([true])
			const palette = document.createElement("div")
			palette.setAttribute("role", "dialog")
			palette.setAttribute("data-open", "")
			palette.setAttribute("data-slot", "dialog-content")
			palette.className = "devo-command-palette"
			const paletteInput = document.createElement("input")
			palette.append(paletteInput)
			document.body.append(palette)
			const competingDialog = document.createElement("div")
			competingDialog.setAttribute("role", "dialog")
			competingDialog.setAttribute("data-open", "")
			document.body.append(competingDialog)
			expect(key(paletteInput, "k", { ctrlKey: true }).defaultPrevented).toBe(false)
			expect(toggles).toEqual([true])
			competingDialog.remove()
			expect(key(paletteInput, "k", { ctrlKey: true }).defaultPrevented).toBe(true)
			expect(toggles).toEqual([true, false])
		} finally {
			document.removeEventListener("keydown", onPaletteKeyDown)
		}
	})

	test("open popovers and focused controls keep their own keys", () => {
		const button = document.createElement("button")
		button.innerHTML = "<span>open</span>"
		document.body.append(button)
		button.focus()
		expect(key(button.firstElementChild!, "Escape").defaultPrevented).toBe(false)
		expect(key(button, "j").defaultPrevented).toBe(false)
		expect(navigations).toEqual([])
		button.blur()
		const popover = document.createElement("div")
		popover.setAttribute("data-slot", "popover-content")
		popover.setAttribute("data-open", "")
		document.body.append(popover)
		expect(key(popover, "k").defaultPrevented).toBe(false)
		expect(navigations).toEqual([])
		popover.remove()
		expect(key(document.body, "j").defaultPrevented).toBe(true)
		expect(navigations).toEqual(["two"])
	})
})
