import { afterEach, expect, mock, test } from "bun:test"
import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/dialog", () => ({
	Dialog: wrap, DialogContent: wrap, DialogDescription: wrap,
	DialogHeader: wrap, DialogTitle: wrap,
}))
mock.module("@devo/ui/components/select", () => ({
	Select: wrap, SelectContent: wrap, SelectGroup: wrap,
	SelectItem: wrap, SelectTrigger: wrap, SelectValue: () => null,
}))
mock.module("@devo/ui/components/switch", () => ({
	Switch: ({ checked, onCheckedChange, disabled, "aria-label": label }: {
		checked: boolean; onCheckedChange: (value: boolean) => void
		disabled?: boolean; "aria-label"?: string
	}) => <button aria-label={label} disabled={disabled}
		onClick={() => onCheckedChange(!checked)}>{checked ? "Enabled" : "Disabled"}</button>,
}))
const upsert = mock(async (_params: unknown) => ({}))
const remove = mock(async (_params: unknown) => ({}))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({ provider: { upsert, modelRemove: remove } }),
}))
const { createRoot } = await import("react-dom/client")
const { ModelEditDialog } = await import("./model-edit-dialog")
const provider: CatalogProviderInfo = {
	id: "gateway", name: "Gateway", enabled: true,
	baseUrl: "https://gateway.example/v1", wireApis: ["openai_chat_completions"],
	modelOverrides: { untouched: { name: "Existing user title" } },
	models: {
		remote: { name: "Remote model", origin: "remote", enabled: true,
			baseUrl: "https://model.example/v1", wireApi: "openai_responses",
			requestModel: "upstream-id", contextWindow: 100000 },
		untouched: { name: "Other catalog model", origin: "remote" },
	},
}
let root: Root | undefined
let mount: HTMLElement
const saved = mock(() => {})
const close = mock((_open: boolean) => {})
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	upsert.mockClear(); remove.mockClear(); saved.mockClear(); close.mockClear()
})
async function renderEditor() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<ModelEditDialog provider={provider}
		modelId="remote" model={provider.models?.remote} open
		onOpenChange={close} onSaved={saved} />))
}
async function click(text: string) {
	const button = [...mount.querySelectorAll("button")].find(button => button.textContent === text)
	if (!button) throw new Error(`Missing button: ${text}`)
	await act(async () => button.click())
}
test("saving a remote model sends only changed metadata and pins its route", async () => {
	const original = structuredClone(provider)
	await renderEditor()
	await click("Enabled")
	const name = mount.querySelector<HTMLInputElement>("#model-name")!
	await act(async () => {
		Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!.call(name, "My remote")
		name.dispatchEvent(new window.Event("input", { bubbles: true }))
	})
	await click("Save")
	expect(upsert.mock.calls).toEqual([[{ provider: expect.objectContaining({
		id: "gateway", wireApis: ["openai_chat_completions", "openai_responses"],
		modelOverrides: { remote: { name: "My remote", enabled: false } },
		models: { remote: {
			origin: "remote", baseUrl: "https://model.example/v1",
			wireApi: "openai_responses", requestModel: "upstream-id",
		} },
	}) }]])
	expect({ removed: remove.mock.calls, saved: saved.mock.calls, closed: close.mock.calls })
		.toEqual({ removed: [], saved: [[]], closed: [[false]] })
	expect(provider).toEqual(original)
})
test("opening and cancelling a catalog model does not persist any catalog entries", async () => {
	await renderEditor()
	await click("Cancel")
	expect({ upserts: upsert.mock.calls, removed: remove.mock.calls, saved: saved.mock.calls })
		.toEqual({ upserts: [], removed: [], saved: [] })
})

test("saving unchanged remote controls does not freeze inherited metadata", async () => {
	await renderEditor()
	await click("Save")
	expect(upsert.mock.calls).toEqual([[{ provider: expect.objectContaining({
		modelOverrides: {},
		models: { remote: {
			origin: "remote", baseUrl: "https://model.example/v1",
			wireApi: "openai_responses", requestModel: "upstream-id",
		} },
	}) }]])
})
