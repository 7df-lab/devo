import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"

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
const upsert = mock(async (_params: unknown) => ({}))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({ provider: { upsert } }),
}))
const { createRoot } = await import("react-dom/client")
const { CustomProviderDialog } = await import("./custom-provider-dialog")
let root: Root | undefined
let mount: HTMLElement
const onSaved = mock(() => {})
const onOpenChange = mock((_open: boolean) => {})

async function render() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(<CustomProviderDialog open onOpenChange={onOpenChange} onSaved={onSaved} />),
	)
}
async function click(label: string) {
	const button = [...document.querySelectorAll("button")].find(
		(button) => button.textContent === label,
	)
	if (!button) throw new Error(`Missing ${label}`)
	await act(async () => {
		button.focus()
		button.click()
	})
}
async function fill(id: string, value: string) {
	const input = document.getElementById(id) as HTMLInputElement
	await act(async () => {
		Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value)
		input.dispatchEvent(new Event("input", { bubbles: true }))
	})
}
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	upsert.mockReset()
	upsert.mockImplementation(async () => ({}))
	onSaved.mockClear()
	onOpenChange.mockClear()
})

test("required ID feedback is outside the scrolling fields and focuses the invalid input", async () => {
	await render()
	await click("Add Provider")
	const alert = document.querySelector('[role="alert"]')
	const input = document.getElementById("provider-id")
	expect(alert?.textContent).toBe("Provider ID is required")
	expect(input?.closest(".overflow-y-auto")?.contains(alert)).toBe(false)
	expect(alert?.nextElementSibling?.getAttribute("data-slot")).toBe("dialog-footer")
	expect(document.activeElement).toBe(input)
	expect(input?.getAttribute("aria-invalid")).toBe("true")
	expect(input?.getAttribute("aria-describedby")).toBe(alert?.id)
	expect(upsert).not.toHaveBeenCalled()
	await click("Add Provider")
	expect(document.activeElement).toBe(input)
	expect(upsert).not.toHaveBeenCalled()
})

test("missing URL feedback moves focus to Base URL without sending a request", async () => {
	await render()
	await fill("provider-id", "custom-test")
	await click("Add Provider")
	const alert = document.querySelector('[role="alert"]')
	const input = document.getElementById("base-url")
	expect(alert?.textContent).toBe("Base URL is required")
	expect(document.activeElement).toBe(input)
	expect(input?.getAttribute("aria-invalid")).toBe("true")
	expect(input?.getAttribute("aria-describedby")).toBe(alert?.id)
	expect(document.getElementById("provider-id")?.getAttribute("aria-invalid")).toBe("false")
	expect(upsert).not.toHaveBeenCalled()
})

test("server errors stay outside the fields and clear on a successful explicit retry", async () => {
	upsert.mockImplementationOnce(async () => {
		throw new Error("Provider save rejected")
	})
	await render()
	await fill("provider-id", "custom-test")
	await fill("base-url", "https://provider.example/v1")
	await click("Add Provider")
	const alert = document.querySelector('[role="alert"]')
	expect(alert?.textContent).toBe("Provider save rejected")
	expect(document.getElementById("base-url")?.closest(".overflow-y-auto")?.contains(alert)).toBe(
		false,
	)
	expect(onSaved).not.toHaveBeenCalled()
	expect(onOpenChange).not.toHaveBeenCalled()
	await click("Add Provider")
	expect(document.querySelector('[role="alert"]')).toBeNull()
	expect(upsert).toHaveBeenCalledTimes(2)
	expect(onSaved).toHaveBeenCalledTimes(1)
	expect(onOpenChange).toHaveBeenCalledWith(false)
})

test("long server feedback is bounded without moving the action footer into the fields", async () => {
	const message = "Provider endpoint rejected this request. ".repeat(100)
	upsert.mockImplementationOnce(async () => {
		throw new Error(message)
	})
	await render()
	await fill("provider-id", "custom-test")
	await fill("base-url", "https://provider.example/v1")
	await click("Add Provider")
	const alert = document.querySelector('[role="alert"]')
	expect(alert?.textContent).toBe(message)
	expect(alert?.classList.contains("max-h-24")).toBe(true)
	expect(alert?.classList.contains("overflow-y-auto")).toBe(true)
	expect(alert?.classList.contains("shrink-0")).toBe(true)
	expect(alert?.nextElementSibling?.getAttribute("data-slot")).toBe("dialog-footer")
	expect(onSaved).not.toHaveBeenCalled()
	await click("Cancel")
	expect(onOpenChange).toHaveBeenCalledWith(false)
})

test("cancel remains available after validation without saving", async () => {
	await render()
	await click("Add Provider")
	await click("Cancel")
	expect(onOpenChange).toHaveBeenCalledWith(false)
	expect(upsert).not.toHaveBeenCalled()
	expect(onSaved).not.toHaveBeenCalled()
})
