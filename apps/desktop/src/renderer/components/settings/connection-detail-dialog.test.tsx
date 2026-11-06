import { afterEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"
import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"

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
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/dialog", () => ({
	Dialog: wrap,
	DialogContent: wrap,
	DialogDescription: wrap,
	DialogFooter: wrap,
	DialogHeader: wrap,
	DialogTitle: wrap,
}))
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }))
mock.module("./model-edit-dialog", () => ({ ModelEditDialog: () => null }))
const discoveredModels = { fresh: { name: "Fresh model", wireApi: "openai_responses" as const, baseUrl: "https://gateway.example/v1", requestModel: "upstream", cost: { input: 2, output: 3 }, capabilities: { vision: true }, contextWindow: 100000 } }
const discover = mock(async (_params: unknown) => ({ providerId: "openai-codex", models: discoveredModels }))
const upsert = mock(async (_params: unknown) => ({}))
const modelRemove = mock(async (_params: unknown) => {})
const refreshCatalog = mock(async (_params: unknown) => ({
	data: { status: "updated" },
}))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({
		provider: { discover, modelRemove, upsert },
		model: { refreshCatalog },
	}),
}))
const { createRoot } = await import("react-dom/client")
const { ConnectionDetailDialog } = await import("./connection-detail-dialog")
const provider: CatalogProviderInfo = {
	id: "openai-codex",
	name: "ChatGPT",
	enabled: true,
	wireApis: ["openai_responses"],
	models: { luna: { name: "Luna" } },
}
let root: Root | undefined
let mount: HTMLElement
const changed = mock(() => {})
async function renderDialog(id = "openai-codex") {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(
			<ConnectionDetailDialog
				provider={{ ...provider, id }}
				connectionModels={{
					custom: { name: "Custom", origin: "user" },
					remote: { name: "Discovered", origin: "remote" },
					legacy: { name: "Legacy discovered" },
				}}
				open
				onOpenChange={() => {}}
				onChanged={changed}
			/>,
		),
	)
}
async function click(label: string) {
	const button = [...mount.querySelectorAll("button")].find((button) =>
		button.textContent?.includes(label),
	)
	if (!button) throw new Error(`Missing button: ${label}`)
	await act(async () => button.click())
}
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	discover.mockReset()
	discover.mockImplementation(async () => ({ providerId: "openai-codex", models: discoveredModels }))
	upsert.mockReset()
	modelRemove.mockReset()
	refreshCatalog.mockReset()
	changed.mockReset()
})
describe("connected provider details", () => {
	test("shows inherited models alongside saved models without offering to delete templates", async () => {
		await renderDialog()
		expect({
			count: mount.textContent?.includes("4 models available."),
			inherited: mount.textContent?.includes("Luna"),
			removeLabels: [...mount.querySelectorAll("button[aria-label]")].map(
				(button) => button.getAttribute("aria-label"),
			),
		}).toEqual({
			count: true,
			inherited: true,
			removeLabels: ["Remove saved model Custom"],
		})
	})
	test("only a manually added model can be removed", async () => {
		await renderDialog()
		await act(async () =>
			mount
				.querySelector<HTMLButtonElement>(
					'[aria-label="Remove saved model Custom"]',
				)
				?.click(),
		)
		expect({
			calls: modelRemove.mock.calls,
			changed: changed.mock.calls.length,
		}).toEqual({
			calls: [[{ providerId: "openai-codex", modelId: "custom" }]],
			changed: 1,
		})
	})
	test("Codex discovery displays temporary models without changing configuration", async () => {
		refreshCatalog.mockImplementation(async () => ({
			data: { status: "updated" },
		}))
		await renderDialog()
		await click("Discover")
		expect({
			refresh: refreshCatalog.mock.calls,
			discover: discover.mock.calls,
			changed: changed.mock.calls.length,
		}).toEqual({
			refresh: [],
			discover: [[{ providerId: "openai-codex", forceRefresh: true }]],
			changed: 0,
		})
		expect(mount.textContent).toContain("Fresh model")
		expect(upsert.mock.calls).toEqual([])
	})
	test("failed discovery is visible and can be retried", async () => {
		discover.mockImplementationOnce(async () => {
			throw new Error("Provider unavailable")
		})
		discover.mockImplementation(async () => ({ providerId: "settings-qa", models: discoveredModels }))
		await renderDialog("settings-qa")
		await click("Discover")
		expect(mount.querySelector('[role="alert"]')?.textContent).toBe(
			"Provider unavailable",
		)
		await click("Discover")
		expect({
			alert: mount.querySelector('[role="alert"]'),
			attempts: discover.mock.calls.length,
			changed: changed.mock.calls.length,
		}).toEqual({ alert: null, attempts: 2, changed: 0 })
	})
	test("saving a discovered model persists only the explicit selection and preserves routing", async () => {
		await renderDialog()
		await click("Discover")
		await click("Save")
		expect(upsert.mock.calls).toEqual([[{ provider: {
			...provider,
			models: { fresh: { origin: "user", baseUrl: "https://gateway.example/v1", wireApi: "openai_responses", requestModel: "upstream" } },
		} }]])
		expect(changed.mock.calls.length).toBe(1)
	})

})
