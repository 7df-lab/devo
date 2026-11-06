import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
const writes = mock(() => {})
const network = mock(() => Promise.reject(new Error("Unexpected network")))
Object.assign(dom, { devo: { providerOAuth: { login: writes } }, fetch: network })
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/button", () => ({ Button: ({ children, onClick }: any) => <button onClick={onClick}>{children}</button> }))
mock.module("@devo/ui/components/input", () => ({ Input: (props: any) => <input {...props} /> }))
mock.module("@devo/ui/components/spinner", () => ({ Spinner: () => null }))
mock.module("@devo/ui/components/skeleton", () => ({ Skeleton: () => null }))
mock.module("@devo/ui/components/dialog", () => ({ Dialog: () => null, DialogContent: wrap, DialogDescription: wrap, DialogFooter: wrap, DialogHeader: wrap, DialogTitle: wrap }))
let catalog: any = { providers: [], connectedIds: new Set(), templateIds: new Set(), connectionModels: {} }
let error: string | null = null
let loading = false
mock.module("../../hooks/use-devo-data", () => ({ useProviderCatalog: () => ({ data: catalog, loading, error, reload: () => {} }) }))
mock.module("../../hooks/use-model-directory", () => ({ useModelDirectory: () => ({ refreshCatalog: () => {}, refreshing: false }) }))
mock.module("../../hooks/use-server", () => ({ useServerConnection: () => ({ connected: true }) }))
mock.module("../../lib/invalidate-provider-queries", () => ({ invalidateProviderDependentQueries: writes }))
mock.module("../../services/connection-manager", () => ({ getBaseClient: () => ({ provider: { disconnect: writes } }) }))
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }))
mock.module("./connect-provider-dialog", () => ({ ConnectProviderDialog: ({ provider }: any) => <div role="dialog" data-provider={provider.id} data-models={JSON.stringify(provider.models)}>OAuth login</div> }))
mock.module("./connection-detail-dialog", () => ({ ConnectionDetailDialog: () => <div role="dialog">Discover and save</div> }))
mock.module("./template-connect-dialog", () => ({ TemplateConnectDialog: () => null }))
mock.module("./custom-provider-dialog", () => ({ CustomProviderDialog: () => null }))
const { ProviderSettings } = await import("./provider-settings")
const { ProviderSetupStep } = await import("../onboarding/steps/provider-setup-step")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let mount: HTMLElement
async function render(node: ReactNode) {
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
	await act(async () => root?.render(node))
}
afterEach(async () => {
	await act(async () => root?.unmount()); mount?.remove()
	catalog = { providers: [], connectedIds: new Set(), templateIds: new Set(), connectionModels: {} }
	error = null; loading = false; writes.mockClear(); network.mockClear()
})
for (const surface of ["settings", "onboarding"] as const) {
	test(`${surface}: empty directory retains all login actions without render side effects`, async () => {
		await render(surface === "settings" ? <ProviderSettings /> : <ProviderSetupStep onComplete={() => {}} onSkip={() => {}} />)
		const section = mount.querySelector('[aria-label="Account sign-in"]')!
		expect([...section.querySelectorAll("button")].map(button => button.textContent)).toEqual([
			"Sign in with ChatGPT", "Sign in with Claude Pro/Max", "Sign in with GitHub Copilot", "Sign in with xAI",
		])
		expect({ writes: writes.mock.calls, network: network.mock.calls, providers: catalog.providers }).toEqual({ writes: [], network: [], providers: [] })
		expect(mount.querySelector('[role="dialog"]')).toBeNull()
		await act(async () => (section.querySelector("button") as HTMLElement).click())
		const dialog = mount.querySelector('[role="dialog"]')!
		expect({ id: dialog.getAttribute("data-provider"), models: dialog.getAttribute("data-models"), providers: catalog.providers }).toEqual({ id: "openai-codex", models: "{}", providers: [] })
	})
}
test("Settings retains login when the directory fails", async () => {
	error = "Offline"
	await render(<ProviderSettings />)
	await act(async () => (mount.querySelector('[aria-label="Account sign-in"] button') as HTMLElement).click())
	expect(mount.querySelector('[role="dialog"]')?.getAttribute("data-provider")).toBe("openai-codex")
})
test("a saved OAuth-only connection absent from the remote directory can open discovery", async () => {
	catalog = { providers: [{ id: "openai-codex", name: "My Codex", models: {}, enabled: true, wireApis: [] }],
		connectedIds: new Set(["openai-codex"]), templateIds: new Set(), connectionModels: {} }
	await render(<ProviderSettings />)
	const button = [...mount.querySelectorAll("button")].find(button => button.textContent?.includes("My Codex"))!
	await act(async () => button.click())
	expect(mount.querySelector('[role="dialog"]')?.textContent).toBe("Discover and save")
	expect(writes.mock.calls).toEqual([])
})

for (const outcome of ["loaded", "error"] as const) {
	test(`Settings keeps an active login through loading → ${outcome}`, async () => {
		loading = true
		await render(<ProviderSettings />)
		await act(async () => (mount.querySelector('[aria-label="Account sign-in"] button') as HTMLElement).click())
		const dialog = mount.querySelector('[role="dialog"]')!
		expect(dialog.getAttribute("data-provider")).toBe("openai-codex")
		loading = false
		error = outcome === "error" ? "Offline" : null
		await act(async () => root?.render(<ProviderSettings />))
		expect(mount.querySelector('[role="dialog"]')).toBe(dialog)
		expect(dialog.getAttribute("data-models")).toBe("{}")
	})
}
