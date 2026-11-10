import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type { ChatTurn } from "../../hooks/use-session-chat"

const dom = new Window({ url: "http://localhost" })
dom.document.write("<!doctype html><html><body></body></html>")
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLTextAreaElement: dom.HTMLTextAreaElement,
	KeyboardEvent: dom.KeyboardEvent,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { StickToBottom } = await import("use-stick-to-bottom")
const { ChatTurnComponent } = await import("./chat-turn")
const turn: ChatTurn = {
	id: "item-user-original",
	turnId: "turn-original",
	userMessage: {
		info: {
			id: "item-user-original",
			sessionId: "session-edit",
			turnId: "turn-original",
			seq: 1,
			revision: 1,
			createdAt: "2026-11-10T00:00:00Z",
			updatedAt: "2026-11-10T00:00:00Z",
			state: "completed",
			item: {
				type: "userMessage",
				content: [{ type: "text", text: "Original message" }],
				entry: "turnStart",
			},
		},
	},
	assistantMessages: [],
}
let root: Root | undefined
let mount: HTMLElement

async function renderTurn(isWorking = false, save?: () => Promise<void>) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	const edits: { itemId: string; text: string }[] = []
	await act(async () => {
		root?.render(
			<StickToBottom>
				<ChatTurnComponent
					turn={turn}
					isLast
					isWorking={isWorking}
					onEditUserMessage={async (text) => {
						// ChatView has already bound this turn's canonical item ID.
						edits.push({ itemId: turn.userMessage.info.id, text })
						await save?.()
					}}
				/>
			</StickToBottom>,
		)
	})
	return edits
}

function button(label: string) {
	const result = [...mount.querySelectorAll<HTMLButtonElement>("button")].find(
		(element) => element.getAttribute("aria-label") === label || element.textContent === label,
	)
	if (!result) throw new Error(`Missing button: ${label}`)
	return result
}

async function edit(text: string) {
	await act(async () => button("Edit message").click())
	const textarea = mount.querySelector<HTMLTextAreaElement>("textarea")
	if (!textarea) throw new Error("Missing message editor")
	await act(async () => {
		const setValue = Object.getOwnPropertyDescriptor(
			dom.HTMLTextAreaElement.prototype,
			"value",
		)?.set
		if (!setValue) throw new Error("Missing native textarea value setter")
		setValue.call(textarea, text)
		textarea.dispatchEvent(new dom.Event("input", { bubbles: true }) as unknown as Event)
	})
	return textarea
}

afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
})

describe("chat turn message edit callback", () => {
	for (const isWorking of [false, true]) {
		test(`Send passes edited text, not the item ID (${isWorking ? "working" : "idle"})`, async () => {
			const edits = await renderTurn(isWorking)
			await edit("  Revised prompt\nKeep this second line  ")
			await act(async () => button("Send").click())
			expect(edits).toEqual([
				{
					itemId: "item-user-original",
					text: "Revised prompt\nKeep this second line",
				},
			])
			expect(mount.querySelector("textarea")).toBeNull()
		})
	}

	test("Enter submits the changed draft once", async () => {
		const edits = await renderTurn()
		const textarea = await edit("Keyboard revision")
		await act(async () => {
			textarea.dispatchEvent(
				new dom.KeyboardEvent("keydown", {
					key: "Enter",
					bubbles: true,
				}) as unknown as Event,
			)
		})
		expect(edits).toEqual([{ itemId: "item-user-original", text: "Keyboard revision" }])
	})

	test("keeps the submitted draft visible and disabled until the edit settles", async () => {
		let resolve!: () => void
		const pending = new Promise<void>((done) => {
			resolve = done
		})
		const edits = await renderTurn(false, () => pending)
		await edit("Pending revision")
		await act(async () => button("Send").click())
		expect({
			text: mount.querySelector<HTMLTextAreaElement>("textarea")?.value,
			textareaDisabled: mount.querySelector<HTMLTextAreaElement>("textarea")?.disabled,
			sendDisabled: button("Sending...").disabled,
			cancelDisabled: button("Cancel").disabled,
		}).toEqual({
			text: "Pending revision",
			textareaDisabled: true,
			sendDisabled: true,
			cancelDisabled: true,
		})
		await act(async () => button("Sending...").click())
		expect(edits).toEqual([{ itemId: "item-user-original", text: "Pending revision" }])
		await act(async () => resolve())
		expect(mount.querySelector("textarea")).toBeNull()
	})

	test("blank drafts cannot be sent, and Cancel restores the original on reopening", async () => {
		const edits = await renderTurn()
		await edit("   ")
		expect(button("Send").disabled).toBe(true)
		await act(async () => button("Cancel").click())
		await act(async () => button("Edit message").click())
		expect(mount.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe("Original message")
		expect(edits).toEqual([])
	})

	test("Escape cancels without changing the saved message", async () => {
		const edits = await renderTurn()
		const textarea = await edit("Discarded draft")
		await act(async () => {
			textarea.dispatchEvent(
				new dom.KeyboardEvent("keydown", {
					key: "Escape",
					bubbles: true,
				}) as unknown as Event,
			)
		})
		expect(edits).toEqual([])
		expect(mount.querySelector("textarea")).toBeNull()
		expect(mount.textContent).toContain("Original message")
	})
})
