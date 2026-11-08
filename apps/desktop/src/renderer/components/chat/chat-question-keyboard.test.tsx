import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act, type ComponentProps } from "react"
import type { Root } from "react-dom/client"
import type { QuestionAnswer, QuestionRequest } from "../../lib/types"

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
const { createRoot } = await import("react-dom/client")
const { ChatQuestionFlow } = await import("./chat-question")
const { handleSessionNavigationKeyDown } = await import("../root-layout-keyboard")
const request: QuestionRequest = {
	id: "request-1",
	sessionId: "session-1",
	questions: [
		{
			id: "question-1",
			header: "Plan",
			question: "Choose a path",
			isOther: true,
			isSecret: false,
			options: [{ label: "Option A", description: "First path" }],
		},
	],
}
let root: Root | undefined
let mount: HTMLElement
let replies: [string, QuestionAnswer[]][] = []
let rejections: string[] = []

async function renderCard(overrides: Partial<ComponentProps<typeof ChatQuestionFlow>> = {}) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root!.render(
			<ChatQuestionFlow
				questions={[request]}
				onReply={async (id, answers) => {
					replies.push([id, answers])
				}}
				onReject={async (id) => {
					rejections.push(id)
				}}
				{...overrides}
			/>,
		),
	)
	return mount.querySelector<HTMLElement>('section[aria-label="Agent question"]')!
}

async function key(
	target: EventTarget,
	key: string,
	options: Pick<
		KeyboardEventInit,
		"key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey" | "isComposing" | "keyCode"
	> = {},
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

async function answer(card: HTMLElement) {
	await act(async () => {
		card.querySelector<HTMLButtonElement>('[role="radio"]')!.click()
	})
}

afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	replies = []
	rejections = []
})

describe("question card keyboard", () => {
	test("unrelated buttons and inputs cannot advance, submit, skip, or navigate questions", async () => {
		const card = await renderCard()
		await answer(card)
		for (const tag of ["button", "input"] as const) {
			const outside = document.createElement(tag)
			document.body.append(outside)
			outside.focus()
			for (const name of ["Enter", "Enter", "Escape", "ArrowRight", "ArrowLeft"]) {
				expect((await key(outside, name)).defaultPrevented).toBe(false)
			}
		}
		expect(card.textContent).toContain("Choose a path")
		expect({ replies, rejections }).toEqual({ replies: [], rejections: [] })
	})

	test("card Enter reviews then sends exactly one answer; arrows can return to editing", async () => {
		const card = await renderCard()
		await answer(card)
		card.focus()
		expect((await key(card, "Enter")).defaultPrevented).toBe(true)
		expect(card.textContent).toContain("Your answers")
		expect((await key(card, "ArrowLeft")).defaultPrevented).toBe(true)
		expect(card.textContent).toContain("Choose a path")
		await key(card, "ArrowRight")
		await key(card, "Enter")
		expect(replies).toEqual([["request-1", [["Option A"]]]])
	})

	test("focused buttons retain native activation instead of triggering the card's Enter action", async () => {
		const card = await renderCard()
		await answer(card)
		const skip = card.querySelector<HTMLButtonElement>('[aria-label="Skip question"]')!
		skip.focus()
		expect((await key(skip, "Enter")).defaultPrevented).toBe(false)
		expect(card.textContent).toContain("Choose a path")
		// Happy DOM does not synthesize native button activation from keyboard events.
		await act(async () => skip.click())
		expect({ replies, rejections }).toEqual({ replies: [], rejections: ["request-1"] })
	})

	test("IME confirmation does not advance a custom answer, including legacy keyCode 229", async () => {
		const card = await renderCard()
		const input = card.querySelector("input")!
		input.focus()
		await act(async () => {
			Object.getOwnPropertyDescriptor(dom.HTMLInputElement.prototype, "value")!.set!.call(
				input,
				"中文",
			)
			input.dispatchEvent(new dom.Event("input", { bubbles: true }) as unknown as Event)
		})
		for (const options of [{ isComposing: true }, { keyCode: 229 }]) {
			expect((await key(input, "Enter", options)).defaultPrevented).toBe(false)
			expect(card.textContent).toContain("Choose a path")
		}
		expect((await key(input, "Enter")).defaultPrevented).toBe(true)
		expect(card.textContent).toContain("Your answers")
		await key(card, "Enter")
		expect(replies).toEqual([["request-1", [["中文"]]]])
	})

	test("card Escape wins over document capture session navigation but ignores composition", async () => {
		const card = await renderCard()
		const navigations: string[] = []
		const onCapture = (event: KeyboardEvent) => {
			handleSessionNavigationKeyDown(event, {
				agents: [{ id: "session-1", projectSlug: "project" }],
				sessionId: "session-1",
				navigate: (target) => navigations.push(target.to),
			})
		}
		document.addEventListener("keydown", onCapture, { capture: true })
		try {
			card.focus()
			expect((await key(card, "Escape", { isComposing: true })).defaultPrevented).toBe(false)
			expect((await key(card, "Escape")).defaultPrevented).toBe(true)
			expect({ rejections, navigations }).toEqual({ rejections: ["request-1"], navigations: [] })
		} finally {
			document.removeEventListener("keydown", onCapture, { capture: true })
		}
	})

	test("disabled and pending cards cannot be submitted or rejected twice", async () => {
		const card = await renderCard({ disabled: true })
		expect((await key(card, "Escape")).defaultPrevented).toBe(false)
		expect(rejections).toEqual([])
		let finish: (() => void) | undefined
		await renderCard({
			onReply: async (id, answers) => {
				replies.push([id, answers])
				await new Promise<void>((resolve) => {
					finish = resolve
				})
			},
		})
		await answer(card)
		await key(card, "Enter")
		await key(card, "Enter")
		await key(card, "Enter")
		await key(card, "Escape")
		expect({ replies, rejections }).toEqual({
			replies: [["request-1", [["Option A"]]]],
			rejections: [],
		})
		await act(async () => finish?.())
	})
})
