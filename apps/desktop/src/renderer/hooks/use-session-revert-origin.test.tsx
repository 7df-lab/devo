import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { Provider } from "jotai"
import type { Root } from "react-dom/client"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { appStore } from "../atoms/store"
import { itemsFamily } from "../atoms/messages"

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
async function mount(items: NativeItemEnvelope[]) {
	appStore.set(itemsFamily(sessionId), items)
	const container = document.createElement("div")
	document.body.append(container)
	root = createRoot(container)
	await act(async () => root!.render(<Provider store={appStore}><Harness /></Provider>))
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	appStore.set(itemsFamily(sessionId), [])
	document.body.replaceChildren()
	calls.length = 0
})

for (const source of ["agent", "backgroundTask", "schedule"]) {
	test(`Undo restores latest human draft before ${source}, never host text`, async () => {
		await mount([message("1", "Older human"), message("2", "Latest human"), message("3", "Host result ready", source)])
		expect(hook.canUndo).toBe(true)
		let composerDraft: string | undefined
		await act(async () => { composerDraft = await hook.undo() })
		expect(composerDraft).toBe("Latest human")
		expect(calls).toEqual([{ method: "revert", params: { sessionId } }])
	})

	test(`Undo with only ${source} input neither fills a draft nor invokes a mutation`, async () => {
		await mount([message("1", "Host result ready", source)])
		expect(hook.canUndo).toBe(false)
		let composerDraft: string | undefined
		await act(async () => { composerDraft = await hook.undo() })
		expect(composerDraft).toBeUndefined()
		expect(calls).toEqual([])
	})
}

test("ordinary human Undo preserves text and existing revert behavior", async () => {
	await mount([message("1", "Original human draft")])
	expect(hook.canUndo).toBe(true)
	let composerDraft: string | undefined
	await act(async () => { composerDraft = await hook.undo() })
	expect(composerDraft).toBe("Original human draft")
	expect(calls).toEqual([{ method: "revert", params: { sessionId } }])
})
