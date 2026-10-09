import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { atom } from "jotai"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement, ResizeObserver: dom.ResizeObserver,
	IS_REACT_ACT_ENVIRONMENT: true })
Object.assign(dom, { devo: { getAppInfo: async () => ({ version: "0.1.39", isDev: true }), nativeTraffic: { getState: async () => null }, restartDevo: async () => { throw new Error("Runtime start failed") } } })
mock.module("../../atoms/preferences", () => ({ opaqueWindowsAtom: atom(false) }))
mock.module("../../hooks/use-agents", () => ({ useDisplayMode: () => "default",
	useSetDisplayMode: () => () => {}, useHideThinkingWhileWorking: () => true,
	useSetHideThinkingWhileWorking: () => () => {} }))
mock.module("../../hooks/use-theme", () => ({ useColorScheme: () => "system", useSetColorScheme: () => () => {} }))
mock.module("../../services/backend", () => ({ isElectron: true,
	fetchOpenInTargets: async () => ({ preferredTarget: "finder", targets: [
		{ id: "finder", label: "File Explorer", available: true },
		{ id: "zed", label: "Zed", available: true } ] }), setOpenInPreferred: async () => {} }))
mock.module("../../hooks/use-updater", () => ({ useUpdater: () => ({ status: "idle", checkForUpdates: () => {} }) }))
let proxy = { mode: "custom", proxyUrl: "not-a-proxy", noProxy: "localhost" }
mock.module("../../hooks/use-settings", () => ({ useSettings: () => ({ settings: { servers: { networkProxy: proxy } }, updateSettings: () => {} }) }))
const { ServerSettings } = await import("./server-settings")
const { createRoot } = await import("react-dom/client")
const { GeneralSettings } = await import("./general-settings")
const { AboutSettings } = await import("./about-settings")
let root: Root | undefined
let mount: HTMLElement
afterEach(async () => { await act(async () => root?.unmount()); mount?.remove() })
test("closed open-destination control shows File Explorer instead of its internal ID", async () => {
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
	await act(async () => root?.render(<GeneralSettings />))
	expect(mount.querySelector('[aria-label="Default open destination"] [data-slot="select-value"]')?.textContent).toBe("File Explorer")
})
test("development builds explain and disable update checks", async () => {
	mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
	await act(async () => root?.render(<AboutSettings />))
	const button = [...mount.querySelectorAll("button")].find((button) => button.textContent?.includes("Check for updates"))
	expect({ message: mount.textContent?.includes("Updates are available in installed builds."), disabled: button?.disabled })
		.toEqual({ message: true, disabled: true })
})

test("invalid custom proxy blocks restart and runtime failures remain visible", async () => {
    mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount)
    await act(async () => root?.render(<ServerSettings />))
    const restart = () => [...mount.querySelectorAll("button")].find((button) => button.textContent?.includes("Restart"))!
    expect({ invalid: mount.textContent?.includes("Invalid proxy URL"), disabled: restart().disabled }).toEqual({ invalid: true, disabled: true })
    proxy = { ...proxy, proxyUrl: "socks5h://127.0.0.1:7890" }
    await act(async () => root?.render(<ServerSettings />))
    expect(restart().disabled).toBe(false)
    await act(async () => restart().click())
    expect(mount.querySelector('[role="alert"]')?.textContent).toBe("Runtime start failed")
})
