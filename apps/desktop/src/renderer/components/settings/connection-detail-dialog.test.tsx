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
const discover = mock(async (_params: unknown) => {})
const modelRemove = mock(async (_params: unknown) => {})
const refreshCatalog = mock(async (_params: unknown) => ({
	data: { status: "updated" },
}))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => ({
		provider: { discover, modelRemove },
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
	test("Codex refreshes its authenticated provider catalog", async () => {
		refreshCatalog.mockImplementation(async () => ({
			data: { status: "updated" },
		}))
		await renderDialog()
		await click("Refresh catalog")
		expect({
			refresh: refreshCatalog.mock.calls,
			discover: discover.mock.calls,
			changed: changed.mock.calls.length,
		}).toEqual({
			refresh: [],
			discover: [[{ providerId: "openai-codex", forceRefresh: true }]],
			changed: 1,
		})
	})
	test("failed discovery is visible and can be retried", async () => {
		discover.mockImplementationOnce(async () => {
			throw new Error("Provider unavailable")
		})
		discover.mockImplementation(async () => {})
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
		}).toEqual({ alert: null, attempts: 2, changed: 1 })
	})
})
