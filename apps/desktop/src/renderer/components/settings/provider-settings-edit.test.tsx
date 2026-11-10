import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"

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
const savedModels = {
	"qwen:latest": {
		name: "Local Qwen",
		origin: "user" as const,
		contextWindow: 32768,
		requestModel: "org/qwen:latest",
		wireApi: "openai_responses" as const,
	},
}
const provider: CatalogProviderInfo = {
	id: "local",
	name: "Local QA",
	description: "Keep my description",
	enabled: true,
	baseUrl: "http://localhost:11434/v1",
	wireApis: ["openai_chat_completions", "openai_responses"],
	headers: { "X-Local": "qa" },
	options: { custom: true },
	models: { ...savedModels, "catalog-only": { name: "Unsaved template", origin: "remote" } },
}
const catalog = {
	providers: [provider],
	connectedIds: new Set(["local"]),
	templateIds: new Set<string>(),
	connectionModels: { local: savedModels },
}
const reload = mock(() => {})
const upsert = mock(async (_params: unknown) => ({}))
mock.module("../../hooks/use-devo-data", () => ({
	useProviderCatalog: () => ({ data: catalog, loading: false, error: null, reload }),
}))
mock.module("../../lib/invalidate-provider-queries", () => ({
	invalidateProviderDependentQueries: () => {},
}))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({ provider: { upsert } }),
}))
mock.module("./desktop-oauth-actions", () => ({ DesktopOAuthActions: () => null }))
mock.module("./connect-provider-dialog", () => ({ ConnectProviderDialog: () => null }))
mock.module("./template-connect-dialog", () => ({ TemplateConnectDialog: () => null }))
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }))
const { createRoot } = await import("react-dom/client")
const { ProviderSettings } = await import("./provider-settings")
let root: Root | undefined
let mount: HTMLElement
async function render() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<ProviderSettings />))
}
async function click(label: string) {
	const button = [...document.querySelectorAll("button")].find(
		(button) => button.textContent?.trim() === label,
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
async function manage() {
	const button = [...document.querySelectorAll("button")].find((button) =>
		button.textContent?.includes("Local QA"),
	)
	if (!button) throw new Error("Missing configured provider")
	await act(async () => button.click())
	await click("Edit connection")
}
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	upsert.mockReset()
	upsert.mockImplementation(async () => ({}))
	reload.mockClear()
})

test("production Manage opens Edit and preserves model transport and connection metadata", async () => {
	await render()
	await manage()
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain("Edit Custom Provider")
	expect((document.getElementById("provider-id") as HTMLInputElement).disabled).toBe(true)
	expect(
		[...document.querySelectorAll<HTMLInputElement>('[placeholder="model-id"]')].map(
			(input) => input.value,
		),
	).toEqual(["qwen:latest"])
	expect((document.querySelector('[placeholder="model-id"]') as HTMLInputElement).disabled).toBe(
		true,
	)
	await fill("base-url", "http://localhost:1234/v1")
	await click("Save")
	expect(upsert).toHaveBeenCalledWith({
		provider: { ...provider, baseUrl: "http://localhost:1234/v1", models: savedModels },
	})
	expect(reload).toHaveBeenCalledTimes(1)
	expect(document.querySelector('[role="dialog"]')).toBeNull()
})

test("production Add receives existing IDs and cannot replace the connected endpoint", async () => {
	await render()
	await click("Add custom provider")
	await fill("provider-id", "local")
	await fill("base-url", "http://localhost:1234/v1")
	await click("Add Provider")
	expect(document.querySelector('[role="alert"]')?.textContent).toContain("already connected")
	expect(upsert).not.toHaveBeenCalled()
	expect(reload).not.toHaveBeenCalled()
})

test("cancelling Edit returns the next Add flow to an empty unlocked provider", async () => {
	await render()
	await manage()
	await fill("base-url", "http://localhost:1234/v1")
	await click("Cancel")
	await click("Add custom provider")
	expect((document.getElementById("provider-id") as HTMLInputElement).disabled).toBe(false)
	expect((document.getElementById("provider-id") as HTMLInputElement).value).toBe("")
	expect((document.getElementById("base-url") as HTMLInputElement).value).toBe("")
	expect(upsert).not.toHaveBeenCalled()
})

test("a failed explicit connection edit stays open with feedback and no success refresh", async () => {
	upsert.mockImplementationOnce(async () => {
		throw new Error("Save failed")
	})
	await render()
	await manage()
	await fill("base-url", "http://localhost:1234/v1")
	await click("Save")
	expect(document.querySelector('[role="alert"]')?.textContent).toBe("Save failed")
	expect(document.querySelector('[role="dialog"]')?.textContent).toContain("Edit Custom Provider")
	expect(reload).not.toHaveBeenCalled()
})

test("changing the primary wire API keeps protocols used by saved model routes", async () => {
	await render()
	await manage()
	await act(async () => (document.querySelector('[aria-label="Wire API"]') as HTMLElement).click())
	const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(
		(item) => item.textContent === "Anthropic Messages",
	)
	expect(option).toBeDefined()
	await act(async () => option?.click())
	await click("Save")
	expect(upsert).toHaveBeenCalledWith({
		provider: {
			...provider,
			wireApis: ["anthropic_messages", "openai_chat_completions", "openai_responses"],
			models: savedModels,
		},
	})
	expect(reload).toHaveBeenCalledTimes(1)
})
