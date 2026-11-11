import { afterEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act, createRef } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	HTMLTextAreaElement: dom.HTMLTextAreaElement,
	CompositionEvent: dom.CompositionEvent,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { PromptInput, PromptInputProvider, PromptInputTextarea, usePromptInputAttachments } =
	await import("@devo/ui/components/ai-elements/prompt-input")
const { InputGroupTextarea } = await import("@devo/ui/components/input-group")
const { useComposerPopoverNavigation } = await import("./composer-popover")
let root: Root | undefined
let mount: HTMLDivElement
let selected: string[] = []
let closed = 0
let shortcuts = 0
let submitted = 0
let forwardedKeys: string[] = []
let composition: string[] = []

let attachments: ReturnType<typeof usePromptInputAttachments>
function AttachmentState() {
	attachments = usePromptInputAttachments()
	return <span data-attachment-count={attachments.files.length} />
}

function Composer({
	open = true,
	customCallbacks = true,
}: {
	open?: boolean
	customCallbacks?: boolean
}) {
	const navigation = useComposerPopoverNavigation({
		items: ["first", "second"],
		open,
		resetKey: "",
		onSelect: (item) => selected.push(item),
		onClose: () => closed++,
	})
	return (
		<PromptInputProvider>
			<PromptInput
				onSubmit={() => {
					submitted++
				}}
			>
				<PromptInputTextarea
					{...(customCallbacks
						? {
								onCompositionStart: () => {
									composition.push("start")
								},
								onCompositionEnd: () => {
									composition.push("end")
								},
							}
						: {})}
					onBlur={() => composition.push("blur")}
					onKeyDown={(event) => {
						forwardedKeys.push(event.key)
						if (event.key === "Tab" && event.shiftKey) {
							event.preventDefault()
							shortcuts++
							return
						}
						if (navigation.handleKeyDown(event)) return
						if (event.key === "Escape") shortcuts++
					}}
				/>
				<output>{navigation.activeIndex}</output>
				<AttachmentState />
			</PromptInput>
		</PromptInputProvider>
	)
}
async function renderComposer(open = true, customCallbacks = true) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<Composer open={open} customCallbacks={customCallbacks} />))
	const textarea = mount.querySelector("textarea")
	if (!textarea?.form) throw new Error("Expected composer textarea and form")
	textarea.form.requestSubmit = () => submitted++
	return textarea
}
async function key(
	target: HTMLTextAreaElement,
	key: string,
	options: Pick<KeyboardEventInit, "isComposing" | "keyCode" | "shiftKey"> = {},
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
async function compose(target: HTMLTextAreaElement, type: "compositionstart" | "compositionend") {
	await act(async () => {
		target.dispatchEvent(new dom.CompositionEvent(type, { bubbles: true }) as unknown as Event)
	})
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	selected = []
	closed = 0
	shortcuts = 0
	submitted = 0
	composition = []
	forwardedKeys = []
})
describe("composer IME keyboard ownership", () => {
	for (const options of [{ isComposing: true }, { keyCode: 229 }]) {
		test(`leaves suggestion keys to the IME: ${JSON.stringify(options)}`, async () => {
			const textarea = await renderComposer()
			for (const name of ["ArrowDown", "ArrowUp", "Enter", "Tab", "Escape", "Backspace"]) {
				expect((await key(textarea, name, options)).defaultPrevented).toBe(false)
			}
			expect((await key(textarea, "Tab", { ...options, shiftKey: true })).defaultPrevented).toBe(
				false,
			)
			expect({
				selected,
				closed,
				shortcuts,
				submitted,
				forwardedKeys,
				index: mount.querySelector("output")?.textContent,
			}).toEqual({
				selected: [],
				closed: 0,
				shortcuts: 0,
				submitted: 0,
				forwardedKeys: [],
				index: "0",
			})
		})
		test(`does not submit or abort without suggestions: ${JSON.stringify(options)}`, async () => {
			const textarea = await renderComposer(false)
			expect((await key(textarea, "Enter", options)).defaultPrevented).toBe(false)
			expect((await key(textarea, "Escape", options)).defaultPrevented).toBe(false)
			expect({ submitted, shortcuts }).toEqual({ submitted: 0, shortcuts: 0 })
		})
	}
	test("tracks composition with caller callbacks and resumes after the final IME key", async () => {
		const textarea = await renderComposer()
		await compose(textarea, "compositionstart")
		for (const name of ["ArrowDown", "ArrowUp", "Enter", "Tab", "Escape", "Backspace"]) {
			expect((await key(textarea, name)).defaultPrevented).toBe(false)
		}
		await compose(textarea, "compositionend")
		expect((await key(textarea, "Enter", { keyCode: 229 })).defaultPrevented).toBe(false)
		expect({ selected, composition, submitted, forwardedKeys }).toEqual({
			selected: [],
			composition: ["start", "end"],
			submitted: 0,
			forwardedKeys: [],
		})
		await key(textarea, "Enter")
		expect(selected).toEqual(["first"])
	})
	test("recovers when focus leaves before compositionend", async () => {
		const textarea = await renderComposer()
		await act(async () => {
			textarea.focus()
		})
		await compose(textarea, "compositionstart")
		await key(textarea, "Enter")
		expect(selected).toEqual([])
		await act(async () => {
			textarea.blur()
			textarea.focus()
		})
		await key(textarea, "Enter")
		expect({ selected, composition }).toEqual({
			selected: ["first"],
			composition: ["start", "blur"],
		})
	})
	for (const customCallbacks of [false, true]) {
		test(`resumes Enter submission after interrupted composition: custom callbacks ${customCallbacks}`, async () => {
			const textarea = await renderComposer(false, customCallbacks)
			await act(async () => textarea.focus())
			await compose(textarea, "compositionstart")
			await key(textarea, "Enter")
			expect(submitted).toBe(0)

			await act(async () => {
				textarea.blur()
				textarea.focus()
			})
			expect((await key(textarea, "Enter")).defaultPrevented).toBe(true)
			expect({ submitted, composition }).toEqual({
				submitted: 1,
				composition: customCallbacks ? ["start", "blur"] : ["blur"],
			})

			// A fresh IME session still owns its keys after recovery.
			await compose(textarea, "compositionstart")
			await key(textarea, "Enter")
			expect(submitted).toBe(1)
			await compose(textarea, "compositionend")
			await key(textarea, "Enter", { keyCode: 229 })
			expect(submitted).toBe(1)
			await key(textarea, "Enter")
			expect(submitted).toBe(2)
		})
	}

	for (const end of ["blur", "compositionend"]) {
		test(`resumes submission synchronously after ${end}`, async () => {
			const textarea = await renderComposer(false, false)
			await act(async () => textarea.focus())
			await compose(textarea, "compositionstart")
			await act(async () => {
				if (end === "blur") {
					textarea.blur()
					textarea.focus()
				} else {
					textarea.dispatchEvent(
						new dom.CompositionEvent("compositionend", { bubbles: true }) as unknown as Event,
					)
				}
				textarea.dispatchEvent(
					new dom.KeyboardEvent("keydown", {
						key: "Enter",
						bubbles: true,
						cancelable: true,
					}) as unknown as Event,
				)
			})
			expect({ submitted, forwardedKeys }).toEqual({ submitted: 1, forwardedKeys: ["Enter"] })
		})
	}
	test("retains normal navigation, selection, dismissal, and shortcuts", async () => {
		const textarea = await renderComposer()
		await key(textarea, "ArrowUp")
		expect(mount.querySelector("output")?.textContent).toBe("1")
		await key(textarea, "Tab")
		await key(textarea, "ArrowDown")
		await key(textarea, "Enter")
		await key(textarea, "Escape")
		await key(textarea, "Tab", { shiftKey: true })
		expect({ selected, closed, shortcuts, submitted }).toEqual({
			selected: ["second", "first"],
			closed: 1,
			shortcuts: 1,
			submitted: 0,
		})
	})
	test("default prompt callbacks preserve composition and subsequent submission", async () => {
		const textarea = await renderComposer(false, false)
		await compose(textarea, "compositionstart")
		await key(textarea, "Enter")
		await key(textarea, "Escape")
		expect({ submitted, shortcuts, forwardedKeys }).toEqual({
			submitted: 0,
			shortcuts: 0,
			forwardedKeys: [],
		})
		await compose(textarea, "compositionend")
		await key(textarea, "Enter", { keyCode: 229 })
		expect(submitted).toBe(0)
		await key(textarea, "Enter")
		expect(submitted).toBe(1)
	})

	test("composition ref protects keys before React has flushed state", async () => {
		const textarea = await renderComposer(true, false)
		await act(async () => {
			textarea.dispatchEvent(
				new dom.CompositionEvent("compositionstart", {
					bubbles: true,
				}) as unknown as Event,
			)
			textarea.dispatchEvent(
				new dom.KeyboardEvent("keydown", {
					key: "Enter",
					bubbles: true,
					cancelable: true,
				}) as unknown as Event,
			)
		})
		expect({ selected, submitted, forwardedKeys }).toEqual({
			selected: [],
			submitted: 0,
			forwardedKeys: [],
		})
		await compose(textarea, "compositionend")
		await key(textarea, "Enter")
		expect(selected).toEqual(["first"])
	})

	test("IME Backspace preserves attachments while ordinary Backspace removes them", async () => {
		const textarea = await renderComposer(false, false)
		await act(async () => {
			attachments.add([new File(["hello"], "note.txt", { type: "text/plain" })])
		})
		for (const options of [{ isComposing: true }, { keyCode: 229 }]) {
			expect((await key(textarea, "Backspace", options)).defaultPrevented).toBe(false)
		}
		await compose(textarea, "compositionstart")
		await key(textarea, "Backspace")
		expect(attachments.files.map((file) => file.filename)).toEqual(["note.txt"])
		await compose(textarea, "compositionend")
		expect((await key(textarea, "Backspace")).defaultPrevented).toBe(true)
		expect(attachments.files).toEqual([])
	})

	test("input group preserves refs and textarea props", async () => {
		const ref = createRef<HTMLTextAreaElement>()
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
		await act(async () =>
			root?.render(
				<InputGroupTextarea
					ref={ref}
					name="draft"
					placeholder="Write"
					defaultValue="中文"
					className="custom-editor"
					aria-label="Draft"
				/>,
			),
		)
		const textarea = mount.querySelector("textarea")
		expect(ref.current).toBe(textarea)
		expect({
			name: textarea?.name,
			placeholder: textarea?.placeholder,
			value: textarea?.value,
			label: textarea?.getAttribute("aria-label"),
			customClass: textarea?.classList.contains("custom-editor"),
		}).toEqual({
			name: "draft",
			placeholder: "Write",
			value: "中文",
			label: "Draft",
			customClass: true,
		})
	})

	test("ordinary Enter submits and Shift-Enter remains a newline", async () => {
		const textarea = await renderComposer(false)
		expect((await key(textarea, "Enter", { shiftKey: true })).defaultPrevented).toBe(false)
		expect((await key(textarea, "Enter")).defaultPrevented).toBe(true)
		expect(submitted).toBe(1)
	})
})
