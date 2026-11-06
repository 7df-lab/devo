import { afterAll, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { Provider, createStore } from "jotai"
import { act } from "react"
import { createRoot } from "react-dom/client"
import { DEFAULT_SERVER_SETTINGS } from "../../shared/server-config"
import { activeServerIdAtom, serversAtom } from "../atoms/connection"

const dom = new Window({ url: "http://localhost" })
const original = Object.getOwnPropertyDescriptors(globalThis)
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const unsubscribe = mock(() => {})
const updateSettings = mock(async (_settings: { servers: typeof DEFAULT_SERVER_SETTINGS }) => {})
let onSettingsChanged: (() => void) | undefined
Object.assign(dom, {
	devo: {
		getSettings: async () => ({ servers: { activeServerId: "old-remote", servers: [] } }),
		updateSettings,
		onSettingsChanged: (callback: () => void) => {
			onSettingsChanged = callback
			return unsubscribe
		},
	},
})
const { useServerSettingsSync } = await import("./use-servers")
function Sync() {
	useServerSettingsSync()
	return null
}

describe("local server settings sync", () => {
	test("normalizes legacy settings, subscribes to changes and cleans up", async () => {
		const store = createStore()
		const root = createRoot(dom.document.createElement("div") as unknown as HTMLElement)
		await act(async () => {
			root.render(<Provider store={store}><Sync /></Provider>)
		})
		expect(updateSettings.mock.calls).toEqual([[{ servers: DEFAULT_SERVER_SETTINGS }]])
		expect([store.get(serversAtom), store.get(activeServerIdAtom)]).toEqual([
			DEFAULT_SERVER_SETTINGS.servers, DEFAULT_SERVER_SETTINGS.activeServerId,
		])
		store.set(activeServerIdAtom, "stale")
		await act(async () => {
			onSettingsChanged?.()
		})
		expect(store.get(activeServerIdAtom)).toBe(DEFAULT_SERVER_SETTINGS.activeServerId)
		await act(async () => {
			root.unmount()
		})
		expect(unsubscribe.mock.calls).toEqual([[]])
	})
})
afterAll(() => {
	for (const key of ["window", "document", "navigator", "IS_REACT_ACT_ENVIRONMENT"]) {
		if (original[key]) Object.defineProperty(globalThis, key, original[key])
		else Reflect.deleteProperty(globalThis, key)
	}
	void dom.happyDOM.close()
})
