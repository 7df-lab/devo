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

async function render(props: Partial<Parameters<typeof CustomProviderDialog>[0]> = {}) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(
			<CustomProviderDialog open onOpenChange={onOpenChange} onSaved={onSaved} {...props} />,
		),
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

for (const url of [
	"not-a-url",
	"localhost:11434",
	"ftp://localhost/v1",
	"http://",
	"http:localhost",
]) {
	test(`invalid endpoint ${url} stays in the form without saving`, async () => {
		await render()
		await fill("provider-id", "local")
		await fill("base-url", url)
		await click("Add Provider")
		expect(document.querySelector('[role="alert"]')?.textContent).toBe(
			"Base URL must be a valid http:// or https:// URL",
		)
		expect(document.activeElement).toBe(document.getElementById("base-url"))
		expect(document.getElementById("base-url")?.getAttribute("aria-invalid")).toBe("true")
		expect(upsert).not.toHaveBeenCalled()
		expect(onSaved).not.toHaveBeenCalled()
		expect(onOpenChange).not.toHaveBeenCalled()
	})
}

test("adding an existing connection is blocked, then a fresh ID can be saved without a key", async () => {
	await render({ connectedIds: new Set(["local"]) })
	await fill("provider-id", "  local  ")
	await fill("base-url", "http://localhost:11434")
	await click("Add Provider")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("already connected")
	expect(document.activeElement).toBe(document.getElementById("provider-id"))
	expect(upsert).not.toHaveBeenCalled()
	expect(onSaved).not.toHaveBeenCalled()
	await fill("provider-id", "another-local")
	await click("Add Provider")
	expect(upsert).toHaveBeenCalledWith({
		provider: {
			id: "another-local",
			name: "another-local",
			baseUrl: "http://localhost:11434",
			wireApis: ["openai_chat_completions"],
			models: {},
			enabled: true,
		},
	})
	expect(onSaved).toHaveBeenCalledTimes(1)
})

test("connected built-ins cannot be overwritten through the Add flow", async () => {
	await render({
		connectedIds: new Set(["openai"]),
		templateIds: new Set(["openai"]),
	})
	await fill("provider-id", "openai")
	await fill("base-url", "http://localhost:1234/v1")
	await click("Add Provider")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("already connected")
	expect(upsert).not.toHaveBeenCalled()
})

test("explicit Edit still updates the selected connected provider", async () => {
	await render({
		connectedIds: new Set(["local"]),
		provider: {
			id: "local",
			name: "Local",
			baseUrl: "http://localhost:11434/v1",
			wireApis: ["openai_chat_completions"],
			models: { "qwen:latest": {} },
			enabled: true,
		},
	})
	await fill("base-url", "http://localhost:1234/v1")
	await click("Save")
	expect(upsert).toHaveBeenCalledWith({
		provider: {
			id: "local",
			name: "Local",
			baseUrl: "http://localhost:1234/v1",
			wireApis: ["openai_chat_completions"],
			models: { "qwen:latest": { name: undefined } },
			headers: {},
			enabled: true,
		},
	})
	expect(onSaved).toHaveBeenCalledTimes(1)
})

test("the alternate built-in ID suggestion skips existing custom connections", async () => {
	await render({ templateIds: new Set(["openai"]), connectedIds: new Set(["openai-custom"]) })
	await fill("provider-id", "openai")
	await fill("base-url", "http://localhost:1234/v1")
	await click("Add Provider")
	await click("Use openai-custom-2")
	expect((document.getElementById("provider-id") as HTMLInputElement).value).toBe("openai-custom-2")
	expect(upsert).not.toHaveBeenCalled()
	await click("Add Provider")
	expect(upsert).toHaveBeenCalledTimes(1)
	expect(onSaved).toHaveBeenCalledTimes(1)
})
