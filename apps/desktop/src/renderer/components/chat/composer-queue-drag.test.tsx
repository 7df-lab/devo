import { afterAll, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import { createRoot } from "react-dom/client"
import { ComposerStatusStack, type ComposerQueueItem } from "./composer-status-stack"

const dom = new Window({ url: "http://localhost" })
const original = Object.getOwnPropertyDescriptors(globalThis)
Object.assign(globalThis, {
	window: dom, document: dom.document, navigator: dom.navigator,
	IS_REACT_ACT_ENVIRONMENT: true,
})
afterAll(() => {
	for (const key of ["window", "document", "navigator", "IS_REACT_ACT_ENVIRONMENT"]) {
		if (original[key]) Object.defineProperty(globalThis, key, original[key])
		else Reflect.deleteProperty(globalThis, key)
	}
	void dom.happyDOM.close()
})

test("drag retains message identity when the queue changes before drop", async () => {
	const container = dom.document.createElement("div")
	const root = createRoot(container as unknown as HTMLElement)
	const onReorder = mock((_from: string, _to: string) => {})
	const items: ComposerQueueItem[] = ["A", "B", "C", "D"].map((id) => ({ id, text: id, status: "queued" }))
	const render = async (queueItems: ComposerQueueItem[]) => {
		await act(async () => { root.render(<ComposerStatusStack queueItems={queueItems} onReorderQueueItem={onReorder} />) })
	}
	try {
		await render(items)
		const start = new dom.Event("dragstart", { bubbles: true })
		Object.assign(start, { dataTransfer: { effectAllowed: "", setData: mock(() => {}) } })
		await act(async () => { container.querySelectorAll('[draggable="true"]')[1].dispatchEvent(start) })
		await render(items.slice(1))
		await act(async () => {
			container.querySelectorAll('[draggable="true"]')[2].dispatchEvent(new dom.Event("drop", { bubbles: true }))
		})
		expect(onReorder.mock.calls).toEqual([["B", "D"]])
	} finally {
		await act(async () => { root.unmount() })
	}
})
