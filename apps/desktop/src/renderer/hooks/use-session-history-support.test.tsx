import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { createDevoClient, type NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { itemsFamily } from "../atoms/messages"
import { sessionFamily } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { revertSession, unrevertSession } from "../services/devo"

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
const calls: string[] = []
const restored: string[] = []
let client: ReturnType<typeof createDevoClient>
mock.module("../services/connection-manager", () => ({ getProjectClient: () => client }))
mock.module("./use-devo-data", () => ({ useServerCommands: () => [] }))
const { useSessionRevert, useCommands } = await import("./use-commands")
const { useAgentActions } = await import("./use-server")
const { useSessionHistoryShortcuts } = await import("./use-session-history-shortcuts")
const { createRoot } = await import("react-dom/client")
let history: ReturnType<typeof useSessionRevert>
let actions: ReturnType<typeof useAgentActions>
let commands: ReturnType<typeof useCommands>
let root: Root | undefined
const item: NativeItemEnvelope = {
	id: "item",
	sessionId: "session",
	turnId: "turn",
	seq: 1,
	revision: 0,
	createdAt: "2026-11-09T00:00:00Z",
	updatedAt: "2026-11-09T00:00:00Z",
	state: "completed",
	item: {
		type: "userMessage",
		entry: "turnStart",
		content: [{ type: "text", text: "Original prompt" }],
	},
}
function Probe() {
	history = useSessionRevert("/fixture", "session")
	actions = useAgentActions()
	commands = useCommands("/fixture", "session", {
		onUndoTextRestore: (text) => restored.push(text),
	})
	useSessionHistoryShortcuts({
		isActive: true,
		canUndo: history.canUndo,
		canRedo: history.canRedo,
		onUndo: history.undo,
		onRedo: history.redo,
	})
	return <div>Transcript</div>
}
async function render(status: "idle" | "busy", staleRevert = false) {
	appStore.set(itemsFamily("session"), [item])
	appStore.set(sessionFamily("session"), {
		session: { id: "session", ...(staleRevert ? { revert: { itemId: "item" } } : {}) },
		status: { type: status },
		permissions: [],
		questions: [],
		directory: "/fixture",
	})
	const mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(
			<Provider store={appStore}>
				<Probe />
			</Provider>,
		),
	)
}
async function key(target: EventTarget, metaKey: boolean, shiftKey: boolean) {
	const event = new dom.KeyboardEvent("keydown", {
		key: shiftKey ? "Z" : "z",
		ctrlKey: !metaKey,
		metaKey,
		shiftKey,
		bubbles: true,
		cancelable: true,
	})
	await act(async () => {
		target.dispatchEvent(event as unknown as Event)
	})
	return event.defaultPrevented
}
beforeEach(() => {
	client = createDevoClient({
		directory: "/fixture",
		transport: {
			connected: () => true,
			subscribe: () => () => {},
			respond: async () => {},
			request: async (method) => {
				calls.push(method)
				if (method === "session/interrupt") return { interrupted: true }
				throw Error(`Unexpected RPC ${method}`)
			},
		},
	})
})
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	appStore.set(itemsFamily("session"), [])
	appStore.set(sessionFamily("session"), null)
	calls.length = 0
	restored.length = 0
})
for (const status of ["idle", "busy"] as const) {
	test(`${status}: history actions reject without interrupting, restoring input, or changing the transcript`, async () => {
		await render(status)
		expect({
			canUndo: history.canUndo,
			canRedo: history.canRedo,
			isReverted: history.isReverted,
		}).toEqual({ canUndo: false, canRedo: false, isReverted: false })
		for (const action of [history.undo, history.redo])
			await expect(action()).rejects.toThrow("not supported")
		expect({ calls, restored, items: appStore.get(itemsFamily("session")) }).toEqual({
			calls: [],
			restored: [],
			items: [item],
		})
	})
	test(`${status}: transcript and native editor undo/redo keys are not intercepted`, async () => {
		await render(status)
		const input = document.createElement("input")
		const textarea = document.createElement("textarea")
		const editable = document.createElement("div")
		editable.contentEditable = "true"
		const nested = document.createElement("span")
		editable.append(nested)
		document.body.append(input, textarea, editable)
		for (const target of [document.body, input, textarea, nested]) {
			for (const metaKey of [false, true]) {
				for (const shiftKey of [false, true])
					expect(await key(target, metaKey, shiftKey)).toBe(false)
			}
		}
		expect({ calls, restored }).toEqual({ calls: [], restored: [] })
	})
	test(`${status}: legacy action and service callers fail without an abort side effect`, async () => {
		await render(status)
		for (const invoke of [
			() => actions.revert("/fixture", "session", "item"),
			() => actions.unrevert("/fixture", "session"),
			() => revertSession(client, "session"),
			() => unrevertSession(client, "session"),
		])
			await expect(invoke()).rejects.toThrow("not supported")
		expect(calls).toEqual([])
	})
}
test("stale legacy revert metadata does not enable Redo or show a false reverted state", async () => {
	await render("idle", true)
	expect({
		canUndo: history.canUndo,
		canRedo: history.canRedo,
		isReverted: history.isReverted,
		revertInfo: history.revertInfo,
	}).toEqual({ canUndo: false, canRedo: false, isReverted: false, revertInfo: undefined })
})
test("legacy command registry explicitly disables session history and advertises no shortcut", async () => {
	await render("idle")
	const entries = commands.filter((command) => command.name === "undo" || command.name === "redo")
	expect(entries.map(({ name, enabled, shortcut }) => ({ name, enabled, shortcut }))).toEqual([
		{ name: "undo", enabled: false, shortcut: undefined },
		{ name: "redo", enabled: false, shortcut: undefined },
	])
	for (const entry of entries) {
		expect(entry.description).toContain("not supported")
		await expect(entry.execute()).rejects.toThrow("not supported")
	}
	expect({ calls, restored }).toEqual({ calls: [], restored: [] })
})
