import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { atom } from "jotai"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true })
const saved: unknown[] = []
let failure = false
mock.module("../../atoms/preferences", () => ({ lastProjectDirectoryAtom: atom("C:\\work\\project") }))
mock.module("../../atoms/connection", () => ({ serverConnectedAtom: atom(true) }))
mock.module("@tanstack/react-query", () => ({ useQuery: () => ({ data: null, error: null }) }))
mock.module("../../hooks/use-devo-data", () => ({ resolveEffectiveModel: () => null,
	getModelCurrentVariant: () => undefined, getModelVariants: () => [], modelAllowsDefaultVariant: () => true }))
mock.module("../../services/connection-manager", () => ({ getBaseClient: () => null, getProjectClient: () => null }))
mock.module("../../lib/model-config-options", () => ({
	persistRuntimeModelSelection: async (...args: unknown[]) => { saved.push(args); if (failure) throw new Error("Preference unavailable") },
	persistRuntimeModelConfigOption: async (...args: unknown[]) => { saved.push(args) },
}))
mock.module("../chat/model-selector", () => ({ ModelSelector: (props: any) => <div data-presentation={props.presentation}>
	<button onClick={() => props.onSelectModel({ providerID: "session", modelID: "openai-codex/gpt-6-luna" })}>Choose Luna</button>
	<button onClick={() => props.onSelectVariant("high")}>Choose high</button>
	<button onClick={props.onManageProviders}>Manage providers</button>
</div> }))
mock.module("./provider-settings", () => ({ ProviderSettings: () => <div>Connection management</div> }))
mock.module("./model-settings", () => ({ ModelSettings: () => <div>Advanced model management</div> }))
const { ModelProviderSettings } = await import("./model-provider-settings")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let mount: HTMLElement
afterEach(async () => { await act(async () => root?.unmount()); mount?.remove(); saved.length = 0; failure = false })
async function render() {
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
	await act(async () => root?.render(<ModelProviderSettings />))
}
async function click(text: string) {
	await act(async () => [...mount.querySelectorAll("button")].find(button => button.textContent === text)!.click())
}
test("Settings embeds the composer picker and saves model and effort through the shared path", async () => {
	await render()
	expect(mount.querySelector('[data-presentation="inline"]')).not.toBeNull()
	await click("Choose Luna"); await click("Choose high")
	expect(saved).toEqual([
		["C:\\work\\project", { providerID: "session", modelID: "openai-codex/gpt-6-luna" }],
		["C:\\work\\project", "thought_level", "high"],
	])
	expect(mount.querySelector('[role="status"]')?.textContent).toBe("Saved for new chats")
})
test("connection and advanced controls stay in the unified destination", async () => {
	await render(); await click("Manage providers")
	expect(mount.textContent).toContain("Connection management")
	await click("Model configuration")
	expect(mount.textContent).toContain("Advanced model management")
})
test("failed saves are visible and the next successful selection clears the error", async () => {
	await render(); failure = true; await click("Choose Luna")
	expect(mount.querySelector('[role="alert"]')?.textContent).toBe("Preference unavailable")
	failure = false; await click("Choose high")
	expect({ error: mount.querySelector('[role="alert"]'), status: mount.querySelector('[role="status"]')?.textContent })
		.toEqual({ error: null, status: "Saved for new chats" })
})
