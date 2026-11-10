import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { Provider } from "jotai"
import type { Root } from "react-dom/client"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { appStore } from "../atoms/store"
import { itemsFamily } from "../atoms/messages"
import { sessionFamily } from "../atoms/sessions"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const calls: Array<{ method: string; params: unknown }> = []
const client = { session: {
	abort: async (params: unknown) => { calls.push({ method: "abort", params }) },
	revert: async (params: unknown) => { calls.push({ method: "revert", params }) },
	unrevert: async (params: unknown) => { calls.push({ method: "unrevert", params }) },
} }
// Exercise the production hook and atoms; only the external SDK lookup is injected.
mock.module("../services/connection-manager", () => ({ getProjectClient: () => client, getBaseClient: () => null }))
const { useSessionRevert } = await import("./use-commands")
let hook: ReturnType<typeof useSessionRevert>
let root: Root | undefined
const sessionId = "undo-origin-session"
function Harness() {
	hook = useSessionRevert("/repo", sessionId)
	return null
}
function message(id: string, text: string, source?: string): NativeItemEnvelope {
	return {
		id, sessionId, turnId: `turn-${id}`, seq: Number(id), revision: 1,
		createdAt: "2026-11-09T00:00:00Z", updatedAt: "2026-11-09T00:00:00Z", state: "completed",
		item: { type: "userMessage", entry: "turnStart", content: [{ type: "text", text }],
			...(source ? { origin: { source, sender: "worker", eventId: `event-${id}`, delivery: "followUp" } } : {}),
		},
	}
}
async function mount(items: NativeItemEnvelope[], status: "idle" | "busy") {
	appStore.set(itemsFamily(sessionId), items)
	appStore.set(sessionFamily(sessionId), {
		session: { id: sessionId }, status: { type: status },
		permissions: [], questions: [], directory: "/repo",
	})
	const container = document.createElement("div")
	document.body.append(container)
	root = createRoot(container)
	await act(async () => root!.render(<Provider store={appStore}><Harness /></Provider>))
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	appStore.set(itemsFamily(sessionId), [])
	appStore.set(sessionFamily(sessionId), null)
	document.body.replaceChildren()
	calls.length = 0
})

for (const status of ["idle", "busy"] as const) {
	for (const source of ["agent", "backgroundTask", "schedule"]) {
		for (const withHuman of [true, false]) {
			test(`${status}: ${source} history withHuman=${withHuman} keeps Undo and Redo disabled`, async () => {
				const items = [
					...(withHuman ? [message("1", "Latest human")] : []),
					message("2", "Host result ready", source),
				]
				await mount(items, status)
				expect({ canUndo: hook.canUndo, canRedo: hook.canRedo }).toEqual({ canUndo: false, canRedo: false })
				for (const action of [hook.undo, hook.redo])
					await expect(action()).rejects.toThrow("not supported")
				expect({ calls, items: appStore.get(itemsFamily(sessionId)) }).toEqual({ calls: [], items })
			})
		}
	}
}
