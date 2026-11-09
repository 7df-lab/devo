import { afterEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { Provider, createStore } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { reviewPanelOpenAtom, reviewPanelSettingsAtom } from "../atoms/ui"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	KeyboardEvent: dom.KeyboardEvent,
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const agent = {
	id: "A",
	sessionId: "A",
	name: "Session title",
	directory: "/demo",
	projectSlug: "demo",
	status: "idle",
}
mock.module("@tanstack/react-router", () => ({
	useNavigate: () => () => {},
	useParams: () => ({ projectSlug: "demo" }),
}))
mock.module("../hooks/use-session-controller", () => ({
	useSessionController: () => ({
		agent,
		chatTurns: [],
		chatLoading: false,
		handleRenameSession: async () => {},
		undo: async () => {},
		redo: async () => {},
	}),
}))
mock.module("./chat/chat-view", () => ({ ChatView: () => null, ChatInputSection: () => null }))
mock.module("./chat", () => ({ ChatView: () => null }))
mock.module("./review/review-side-rail", () => ({ ReviewSideRail: () => null }))
mock.module("./context-usage-button", () => ({ ContextUsageButton: () => null }))
mock.module("./worktree-actions", () => ({ WorktreeActions: () => null }))
mock.module("../services/backend", () => ({
	isElectron: () => false,
	fetchOpenInTargets: async () => ({ targets: [] }),
	openInTarget: async () => {},
	setOpenInPreferred: async () => {},
}))
const { createRoot } = await import("react-dom/client")
const { SessionShell } = await import("./session-shell")
let root: Root | undefined
let mount: HTMLElement
let store = createStore()

async function renderShell() {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	store = createStore()
	store.set(reviewPanelOpenAtom, false)
	store.set(reviewPanelSettingsAtom, { ...store.get(reviewPanelSettingsAtom), expanded: false })
	await act(async () =>
		root!.render(
			<Provider store={store}>
				<SessionShell activeSessionId="A" />
			</Provider>,
		),
	)
}

async function key(
	key: string,
	options: Pick<
		KeyboardEventInit,
		"key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "isComposing" | "keyCode"
	> = {},
	target: EventTarget = document.body,
) {
	const event = new dom.KeyboardEvent("keydown", {
		key,
		bubbles: true,
		cancelable: true,
		...options,
	})
	await act(async () => {
		target.dispatchEvent(event as unknown as Event)
	})
	return event
}

afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
})

describe("session shell keyboard", () => {
	test("Shift+D and Shift+F work with uppercase or lowercase keys, Ctrl or Meta, and repeated toggles", async () => {
		await renderShell()
		for (const modifier of [{ ctrlKey: true }, { metaKey: true }]) {
			for (const letter of ["D", "d"]) {
				expect((await key(letter, { ...modifier, shiftKey: true })).defaultPrevented).toBe(true)
				expect(store.get(reviewPanelOpenAtom)).toBe(true)
				await key("F", { ...modifier, shiftKey: true })
				expect(store.get(reviewPanelSettingsAtom).expanded).toBe(true)
				await key("f", { ...modifier, shiftKey: true })
				expect(store.get(reviewPanelSettingsAtom).expanded).toBe(false)
				await key(letter, { ...modifier, shiftKey: true })
				expect(store.get(reviewPanelOpenAtom)).toBe(false)
			}
		}
	})

	test("composition and unmodified keys do not toggle review; listeners are removed on unmount", async () => {
		await renderShell()
		for (const options of [
			{ ctrlKey: true, shiftKey: true, isComposing: true },
			{ ctrlKey: true, shiftKey: true, keyCode: 229 },
			{ ctrlKey: true },
			{ shiftKey: true },
		]) {
			expect((await key("D", options)).defaultPrevented).toBe(false)
		}
		expect(store.get(reviewPanelOpenAtom)).toBe(false)
		await act(async () => root?.unmount())
		root = undefined
		expect((await key("D", { ctrlKey: true, shiftKey: true })).defaultPrevented).toBe(false)
		expect(store.get(reviewPanelOpenAtom)).toBe(false)
	})

	test("the real title editor focuses and selects the name on every edit, including after Escape", async () => {
		await renderShell()
		for (let attempt = 0; attempt < 2; attempt++) {
			const title = [...mount.querySelectorAll("button")].find(
				(button) => button.textContent === "Session title",
			)!
			title.focus()
			await act(async () => title.click())
			const input = mount.querySelector("input")!
			expect({
				focused: document.activeElement === input,
				selection: [input.selectionStart, input.selectionEnd],
				value: input.value,
			}).toEqual({ focused: true, selection: [0, "Session title".length], value: "Session title" })
			await key("Escape", {}, input)
			expect(mount.querySelector("input")).toBeNull()
		}
	})
})
