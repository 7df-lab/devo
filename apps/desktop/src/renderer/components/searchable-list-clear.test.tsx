import { afterEach, beforeEach, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, { window: dom, document: dom.document, navigator: dom.navigator,
	HTMLElement: dom.HTMLElement, Element: dom.Element, requestAnimationFrame: dom.requestAnimationFrame.bind(dom), cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom), IS_REACT_ACT_ENVIRONMENT: true })
const { createRoot } = await import("react-dom/client")
const { SearchableListPopover, SearchableListPopoverSearch, useSearchableListPopoverSearch } = await import("@devo/ui/components/searchable-list-popover")
let root: Root
let mount: HTMLDivElement
function Results() {
	const search = useSearchableListPopoverSearch()
	return <output>{["alpha", "beta"].filter((item) => item.includes(search)).join(",")}</output>
}
async function render(open = true) {
	await act(async () => root.render(<SearchableListPopover open={open}>
		<SearchableListPopoverSearch placeholder="Search branches..." /><Results />
	</SearchableListPopover>))
}
async function type(value: string) {
	const input = mount.querySelector("input")!
	await act(async () => {
		Object.getOwnPropertyDescriptor(dom.HTMLInputElement.prototype, "value")!.set!.call(input, value)
		input.dispatchEvent(new dom.Event("input", { bubbles: true }) as unknown as Event)
	})
}
beforeEach(async () => { mount = document.createElement("div"); document.body.append(mount); root = createRoot(mount); await render() })
afterEach(async () => { await act(async () => root.unmount()); mount.remove() })
test("search has an explicit accessible name and empty search has no clear tab stop", () => {
	expect({ label: mount.querySelector("input")?.getAttribute("aria-label"), clear: !!mount.querySelector("button") }).toEqual({ label: "Search branches...", clear: false })
})
test("clear restores all filtered items without closing and returns focus to input", async () => {
	await type("alp")
	expect(mount.querySelector("output")?.textContent).toBe("alpha")
	const button = mount.querySelector("button")!
	expect({ label: button.getAttribute("aria-label"), type: button.type, disabled: button.disabled }).toEqual({ label: "Clear search", type: "button", disabled: false })
	await act(async () => button.click())
	expect({ value: mount.querySelector("input")?.value, results: mount.querySelector("output")?.textContent, focused: document.activeElement === mount.querySelector("input"), clear: !!mount.querySelector("button") }).toEqual({ value: "", results: "alpha,beta", focused: true, clear: false })
})
test("no-results query can be cleared and typing continues normally", async () => {
	await type("missing")
	expect(mount.querySelector("output")?.textContent).toBe("")
	await act(async () => mount.querySelector("button")!.click())
	await type("beta")
	expect(mount.querySelector("output")?.textContent).toBe("beta")
})
test("closing and reopening still resets query and clear control", async () => {
	await type("alpha"); await render(false); await render(true)
	expect({ value: mount.querySelector("input")?.value, clear: !!mount.querySelector("button") }).toEqual({ value: "", clear: false })
})
