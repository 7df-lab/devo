import { afterEach, expect, test } from "bun:test"
import { sessionUsageFromProtocol } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { processEvent } from "../atoms/actions/event-processor"
import { appStore } from "../atoms/store"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom, document: dom.document, navigator: dom.navigator,
	Node: dom.Node, Element: dom.Element, HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement, ShadowRoot: dom.ShadowRoot,
	Event: dom.Event, MouseEvent: dom.MouseEvent, KeyboardEvent: dom.KeyboardEvent,
	MutationObserver: dom.MutationObserver, ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom), IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { SessionMetricsOverviewButton } = await import("./session-metrics-bar")
let root: Root | undefined
let mount: HTMLElement | undefined

afterEach(async () => {
	await act(async () => { root?.unmount(); await Bun.sleep(30) })
	root = undefined
	mount?.remove()
})

test("an open session overview updates after each request without turn completion", async () => {
	const sessionId = "overview-live-requests"
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => root?.render(<Provider store={appStore}><SessionMetricsOverviewButton sessionId={sessionId} /></Provider>))
	await act(async () => {
		document.querySelector<HTMLButtonElement>('[aria-label="Show session overview"]')!.click()
		await Bun.sleep(50)
	})
	const panel = () => document.querySelector('[aria-label="Cumulative token usage"]')!
	expect(panel().textContent).toContain("No usage reported yet.")
	const update = async (totalTokens: number, inputTokens: number, outputTokens: number, callCount: number) => {
		await act(async () => {
			processEvent({ type: "session.usage.updated", properties: { sessionId, usage: sessionUsageFromProtocol({
				totalTokens, inputTokens, outputTokens, callCount, meteredCallCount: callCount, unreportedFields: [],
				reasoningTokens: 0, cacheReadInputTokens: 40, cacheCreationInputTokens: 0,
			})! } })
		})
	}
	await update(120, 100, 20, 1)
	expect(panel().textContent).toContain("Total120")
	expect(panel().textContent).toContain("1 request metered.")
	await update(350, 300, 50, 2)
	expect(panel().textContent).toContain("Input300")
	expect(panel().textContent).toContain("Output50")
	expect(panel().textContent).toContain("Total350")
	expect(panel().textContent).toContain("2 requests metered.")
	expect(document.body.textContent).toContain("Unknown")
})
