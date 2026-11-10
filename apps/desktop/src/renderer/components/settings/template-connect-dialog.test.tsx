import { afterEach, expect, mock, test } from "bun:test"
import {
	type CatalogProviderInfo,
	createDevoClient,
	type DevoNativeTransport,
} from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"

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
const provider: CatalogProviderInfo = {
	id: "deepseek",
	name: "DeepSeek",
	enabled: true,
	baseUrl: "https://api.deepseek.com/v1",
	wireApis: ["openai_chat_completions"],
	models: { chat: { name: "DeepSeek Chat" } },
}
const requests: Array<{ method: string; params: unknown }> = []
let failSave = false
let failDiscover = false
let holdSave: Promise<void> | undefined
const transport: DevoNativeTransport = {
	connected: () => true,
	subscribe: () => () => {},
	respond: async () => {},
	request: async (method, params) => {
		requests.push({ method, params })
		if (method === "initialize")
			return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
		if (method === "provider/upsert") {
			await holdSave
			if (failSave) throw new Error("Could not save configuration")
			return {
				provider: (params as { provider: CatalogProviderInfo }).provider,
				defaultModel: null,
			}
		}
		if (method === "provider/discover") {
			if (failDiscover) throw new Error("Local runtime unavailable")
			return { providerId: "ollama", models: {} }
		}
		throw new Error(`Unexpected RPC ${method}`)
	},
}
let client = createDevoClient({ directory: "/repo", transport: { ...transport } })
mock.module("../../services/connection-manager", () => ({ getBaseClient: () => client }))
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }))
const { createRoot } = await import("react-dom/client")
const { TemplateConnectDialog } = await import("./template-connect-dialog")
const onConnected = mock(() => {})
const onOpenChange = mock((_open: boolean) => {})
let root: Root | undefined
let mount: HTMLElement
async function render(selected = provider) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(
			<TemplateConnectDialog
				provider={selected}
				open
				onOpenChange={onOpenChange}
				onConnected={onConnected}
			/>,
		),
	)
}
async function click(label: string) {
	const button = [...document.querySelectorAll("button")].find(
		(button) => button.textContent === label,
	)
	if (!button) throw new Error(`Missing ${label}`)
	await act(async () => button.click())
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
	requests.length = 0
	failSave = false
	failDiscover = false
	holdSave = undefined
	onConnected.mockClear()
	onOpenChange.mockClear()
	client = createDevoClient({ directory: "/repo", transport: { ...transport } })
})
for (const key of ["", "   "]) {
	test(`DeepSeek rejects ${key ? "whitespace" : "empty"} keys before any SDK request`, async () => {
		await render()
		await fill("api-key", key)
		await click("Connect")
		const alert = document.querySelector('[role="alert"]')
		const input = document.getElementById("api-key")
		expect(alert?.textContent).toBe("API key is required")
		expect(document.activeElement).toBe(input)
		expect(input?.getAttribute("aria-invalid")).toBe("true")
		expect(input?.getAttribute("aria-describedby")).toBe(alert?.id)
		expect(requests).toEqual([])
		await click("Connect")
		expect(requests).toEqual([])
		await click("Cancel")
		expect(onOpenChange).toHaveBeenCalledWith(false)
		expect(onConnected).not.toHaveBeenCalled()
	})
}
test("a saved key is not presented as a model connectivity test", async () => {
	await render()
	await fill("api-key", "  synthetic-test-key  ")
	await click("Connect")
	expect(requests.map((request) => request.method)).toEqual(["initialize", "provider/upsert"])
	expect(requests[1]?.params).toEqual({ provider, apiKey: "synthetic-test-key" })
	expect(document.querySelector("output")?.textContent).toContain("Configuration saved")
	expect(document.querySelector("output")?.textContent).toContain(
		"Model access has not been tested",
	)
	expect(document.body.textContent).not.toContain("Connected —")
	expect(onConnected).not.toHaveBeenCalled()
	await click("Done")
	expect(onConnected).toHaveBeenCalledTimes(1)
	expect(onOpenChange).toHaveBeenCalledWith(false)
})
test("Ollama remains key-optional and failed discovery does not claim verification", async () => {
	failDiscover = true
	await render({ ...provider, id: "ollama", name: "Ollama", baseUrl: "http://localhost:11434/v1" })
	await click("Connect")
	expect(requests.map((request) => request.method)).toEqual([
		"initialize",
		"provider/upsert",
		"provider/discover",
	])
	expect(requests[1]?.params).toEqual({
		provider: {
			...provider,
			id: "ollama",
			name: "Ollama",
			baseUrl: "http://localhost:11434/v1",
			models: {},
		},
	})
	expect(document.querySelector("output")?.textContent).toContain("Configuration saved")
	expect(document.querySelector("output")?.textContent).toContain("Could not refresh models")
	expect(document.body.textContent).not.toContain("Connected —")
	await click("Done")
	expect(onConnected).toHaveBeenCalledTimes(1)
})
test("failed saves remain editable and retry only after another explicit click", async () => {
	failSave = true
	await render()
	await fill("api-key", "synthetic-test-key")
	await click("Connect")
	expect(document.querySelector('[role="alert"]')?.textContent).toBe("Could not save configuration")
	expect(document.querySelector("output")).toBeNull()
	expect(onConnected).not.toHaveBeenCalled()
	failSave = false
	await click("Connect")
	expect(requests.filter((request) => request.method === "provider/upsert")).toHaveLength(2)
	await click("Done")
	expect(onConnected).toHaveBeenCalledTimes(1)
})
test("an empty Base URL is rejected without writing or probing the provider", async () => {
	await render()
	await fill("connect-base-url", "  ")
	await fill("api-key", "synthetic-test-key")
	await click("Connect")
	expect(document.querySelector('[role="alert"]')?.textContent).toBe("Base URL is required")
	expect(document.activeElement?.id).toBe("connect-base-url")
	expect(requests).toEqual([])
})
test("pending saves cannot be submitted twice or dismissed before their outcome", async () => {
	let finish!: () => void
	holdSave = new Promise<void>((resolve) => {
		finish = resolve
	})
	await render()
	await fill("api-key", "synthetic-test-key")
	const connect = [...document.querySelectorAll("button")].find(
		(button) => button.textContent === "Connect",
	)
	if (!connect) throw new Error("Missing Connect")
	await act(async () => {
		connect.click()
		connect.click()
	})
	expect(document.querySelector('[data-slot="dialog-close"]')).toBeNull()
	await click("Saving…")
	await click("Cancel")
	expect(requests.filter((request) => request.method === "provider/upsert")).toHaveLength(1)
	expect(onOpenChange).not.toHaveBeenCalled()
	await act(async () => {
		finish()
		await holdSave
	})
	await click("Done")
	expect(onConnected).toHaveBeenCalledTimes(1)
})

test("closing the saved result with the dialog close control refreshes once without another write", async () => {
	await render()
	await fill("api-key", "synthetic-test-key")
	await click("Connect")
	await click("Close")
	expect(onConnected).toHaveBeenCalledTimes(1)
	expect(onOpenChange).toHaveBeenCalledWith(false)
	expect(requests.filter((request) => request.method === "provider/upsert")).toHaveLength(1)
})

test("a pending response cannot overwrite a different provider dialog", async () => {
	let finish!: () => void
	holdSave = new Promise<void>((resolve) => {
		finish = resolve
	})
	await render()
	await fill("api-key", "synthetic-test-key")
	await click("Connect")
	await act(async () =>
		root?.render(
			<TemplateConnectDialog
				provider={{ ...provider, id: "other", name: "Other" }}
				open
				onOpenChange={onOpenChange}
				onConnected={onConnected}
			/>,
		),
	)
	await act(async () => {
		finish()
		await holdSave
	})
	expect(document.querySelector("output")).toBeNull()
	expect((document.getElementById("api-key") as HTMLInputElement).value).toBe("")
	expect(document.body.textContent).toContain("Connect Other")
	expect(onConnected).not.toHaveBeenCalled()
	await click("Cancel")
	expect(onOpenChange).toHaveBeenCalledWith(false)
})

test("successful local directory discovery is still not reported as a model test", async () => {
	await render({ ...provider, id: "ollama", name: "Ollama", baseUrl: "http://localhost:11434/v1" })
	await click("Connect")
	expect(requests.map((request) => request.method)).toEqual([
		"initialize",
		"provider/upsert",
		"provider/discover",
	])
	expect(document.querySelector("output")?.textContent).toContain(
		"Model access has not been tested",
	)
	expect(document.querySelector("output")?.textContent).not.toContain("Could not refresh models")
	await click("Done")
	expect(onConnected).toHaveBeenCalledTimes(1)
})
