import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, useState } from "react"
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

function Controller({ retained = false, templateIds = new Set<string>() }) {
	const [open, setOpen] = useState(true)
	return (
		<>
			<button type="button" onClick={() => setOpen(true)}>
				Reopen
			</button>
			{(open || retained) && (
				<CustomProviderDialog
					open={open}
					onOpenChange={setOpen}
					onSaved={onSaved}
					templateIds={templateIds}
				/>
			)}
		</>
	)
}
async function controlled(options: { retained?: boolean; templateIds?: Set<string> } = {}) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<Controller {...options} />))
	await fill("provider-id", "custom-test")
	await fill("base-url", "https://provider.example/v1")
}
for (const retained of [false, true]) {
	test(`late successful save preserves the reopened ${retained ? "retained" : "remounted"} form`, async () => {
		let finish!: () => void
		upsert.mockImplementationOnce(
			() =>
				new Promise((resolve) => {
					finish = () => resolve({})
				}),
		)
		await controlled({ retained })
		await click("Add Provider")
		await click("Close")
		await click("Reopen")
		await fill("provider-id", "new-input")
		await act(async () => finish())
		expect(document.querySelector('[role="dialog"]')).not.toBeNull()
		expect((document.getElementById("provider-id") as HTMLInputElement).value).toBe("new-input")
		expect(onSaved).toHaveBeenCalledTimes(1)
	})
}

for (const retained of [false, true]) {
	test(`an obsolete failure cannot overwrite the reopened ${retained ? "retained" : "remounted"} form`, async () => {
		let fail!: (error: Error) => void
		upsert.mockImplementationOnce(
			() =>
				new Promise((_, reject) => {
					fail = reject
				}),
		)
		await controlled({ retained })
		await click("Add Provider")
		await click("Close")
		await click("Reopen")
		await fill("provider-id", "new-input")
		await act(async () => fail(new Error("obsolete failure")))
		expect(document.querySelector('[role="dialog"]')).not.toBeNull()
		expect(document.body.textContent).not.toContain("obsolete failure")
		expect((document.getElementById("provider-id") as HTMLInputElement).value).toBe("new-input")
		expect(onSaved).not.toHaveBeenCalled()
	})
}

test("an old response cannot finish a newer pending save", async () => {
	let finishOld!: () => void
	let finishNew!: () => void
	upsert.mockImplementationOnce(
		() =>
			new Promise((resolve) => {
				finishOld = () => resolve({})
			}),
	)
	upsert.mockImplementationOnce(
		() =>
			new Promise((resolve) => {
				finishNew = () => resolve({})
			}),
	)
	await controlled({ retained: true })
	await click("Add Provider")
	await click("Close")
	await click("Reopen")
	await fill("provider-id", "new-input")
	await click("Add Provider")
	await act(async () => finishOld())
	expect(document.querySelector('[role="dialog"]')).not.toBeNull()
	expect((document.getElementById("provider-id") as HTMLInputElement).disabled).toBe(true)
	expect(document.body.textContent).toContain("Saving…")
	await act(async () => finishNew())
	expect(document.querySelector('[role="dialog"]')).toBeNull()
	expect(onSaved).toHaveBeenCalledTimes(2)
})

test("a successful save after unmount refreshes the catalog without closing another dialog", async () => {
	let finish!: () => void
	upsert.mockImplementationOnce(
		() =>
			new Promise((resolve) => {
				finish = () => resolve({})
			}),
	)
	await render()
	await fill("provider-id", "custom-test")
	await fill("base-url", "https://provider.example/v1")
	await click("Add Provider")
	await act(async () => root?.unmount())
	root = undefined
	await act(async () => finish())
	expect(onOpenChange).not.toHaveBeenCalled()
	expect(onSaved).toHaveBeenCalledTimes(1)
})

test("repeated submit calls start one write and an active failure allows explicit retry", async () => {
	let fail!: (error: Error) => void
	upsert.mockImplementationOnce(
		() =>
			new Promise((_, reject) => {
				fail = reject
			}),
	)
	await controlled()
	const submit = [...document.querySelectorAll("button")].find(
		(button) => button.textContent === "Add Provider",
	)
	if (!submit) throw new Error("Missing Add Provider")
	await act(async () => {
		submit.click()
		submit.click()
	})
	expect(upsert).toHaveBeenCalledTimes(1)
	await act(async () => fail(new Error("active save rejected")))
	expect(document.body.textContent).toContain("active save rejected")
	expect((document.getElementById("provider-id") as HTMLInputElement).disabled).toBe(false)
	await click("Add Provider")
	expect(upsert).toHaveBeenCalledTimes(2)
	expect(document.querySelector('[role="dialog"]')).toBeNull()
	expect(onSaved).toHaveBeenCalledTimes(1)
})

test("a late built-in overlay save cannot dismiss a newer custom form", async () => {
	let finish!: () => void
	upsert.mockImplementationOnce(
		() =>
			new Promise((resolve) => {
				finish = () => resolve({})
			}),
	)
	await controlled({ templateIds: new Set(["custom-test"]) })
	await click("Add Provider")
	await click("Overlay built-in")
	await click("Close")
	await click("Reopen")
	await fill("provider-id", "new-input")
	await act(async () => finish())
	expect(document.querySelector('[role="dialog"]')).not.toBeNull()
	expect((document.getElementById("provider-id") as HTMLInputElement).value).toBe("new-input")
	expect(onSaved).toHaveBeenCalledTimes(1)
})
