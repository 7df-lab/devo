import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, createContext, useContext, useState, type ReactNode } from "react"
import type { Root } from "react-dom/client"
import type { PickerModel } from "./model-picker-data"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	HTMLInputElement: dom.HTMLInputElement,
	Event: dom.Event,
	KeyboardEvent: dom.KeyboardEvent,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const Search = createContext({ value: "", set: (_value: string) => {} })
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/searchable-list-popover", () => ({
	SearchableListPopoverList: wrap,
	SearchableListPopoverEmpty: wrap,
	useSearchableListPopoverSearch: () => useContext(Search).value,
	SearchableListPopoverSearch: () => {
		const search = useContext(Search)
		return (
			<input
				aria-label="Search"
				value={search.value}
				onInput={(event) => search.set(event.currentTarget.value)}
			/>
		)
	},
}))
const { createRoot } = await import("react-dom/client")
const { ModelPickerList } = await import("./model-picker-list")
const models: PickerModel[] = [
	{
		value: "codex/one",
		selection: { providerID: "session", modelID: "codex/one" },
		providerID: "codex",
		providerName: "ChatGPT",
		modelID: "one",
		displayName: "Coding One",
		ready: true,
	},
	{
		value: "other/two",
		selection: { providerID: "session", modelID: "other/two" },
		providerID: "other",
		providerName: "Other Provider",
		modelID: "two",
		displayName: "Coding Two",
		ready: false,
	},
]
let root: Root | undefined
let mount: HTMLElement
const selected: PickerModel[] = []
let refreshes = 0
async function renderPicker(
	options: { models?: PickerModel[]; refreshing?: boolean; error?: string } = {},
) {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	function Harness() {
		const [value, set] = useState("")
		return (
			<Search.Provider value={{ value, set }}>
				<ModelPickerList
					models={options.models ?? models}
					activeValue="codex/one"
					loading={false}
					error={options.error ?? null}
					refreshing={options.refreshing}
					onRetry={() => refreshes++}
					onRefresh={() => refreshes++}
					onSelect={(model) => selected.push(model)}
					onManageProviders={() => {}}
				/>
			</Search.Provider>
		)
	}
	await act(async () => root?.render(<Harness />))
}
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	root = undefined
	selected.length = 0
	refreshes = 0
})

test("ready/all scopes preserve provider identity and exact selections", async () => {
	await renderPicker()
	expect(
		[...mount.querySelectorAll("button[data-model-option]")].map((row) => [
			row.getAttribute("aria-label"),
			row.getAttribute("aria-current"),
		]),
	).toEqual([["Coding One, ChatGPT", "true"]])
	await act(async () =>
		[...mount.querySelectorAll("button")].find((row) => row.textContent === "All models")?.click(),
	)
	const rows = mount.querySelectorAll<HTMLButtonElement>("button[data-model-option]")
	expect([...rows].map((row) => row.getAttribute("aria-label"))).toEqual([
		"Coding One, ChatGPT",
		"Coding Two, Other Provider, connect provider",
	])
	await act(async () => rows[1].click())
	expect(selected).toEqual([models[1]])
})

test("search filters provider/model words and Enter selects a canonical model", async () => {
	await renderPicker()
	const input = mount.querySelector("input")!
	await act(async () => {
		input.value = "CHATGPT coding"
		input.dispatchEvent(new Event("input", { bubbles: true }))
	})
	await act(async () => {
		input.focus()
		input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
	})
	expect(selected).toEqual([models[0]])
	await act(async () => {
		input.value = "missing"
		input.dispatchEvent(new Event("input", { bubbles: true }))
	})
	expect({
		rows: mount.querySelectorAll("button[data-model-option]").length,
		empty: mount.textContent?.includes("No matching models"),
	}).toEqual({ rows: 0, empty: true })
})

test("arrow navigation wraps from search through model rows", async () => {
	await renderPicker()
	const input = mount.querySelector("input")!
	await act(async () => {
		input.focus()
		input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }))
	})
	expect(document.activeElement?.getAttribute("aria-label")).toBe("Coding One, ChatGPT")
	await act(async () =>
		document.activeElement?.dispatchEvent(
			new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }),
		),
	)
	expect(document.activeElement?.getAttribute("aria-label")).toBe("Coding One, ChatGPT")
})

test("limits long catalogs and exposes more models on demand", async () => {
	await renderPicker({
		models: Array.from({ length: 90 }, (_, index) => ({
			...models[0],
			value: `codex/${index}`,
		})),
	})
	expect(mount.querySelectorAll("button[data-model-option]").length).toBe(60)
	await act(async () =>
		[...mount.querySelectorAll("button")]
			.find((row) => row.textContent?.startsWith("Show more"))
			?.click(),
	)
	expect(mount.querySelectorAll("button[data-model-option]").length).toBe(90)
})

test("remote failure retains selectable saved models and supports retry", async () => {
	await renderPicker({ error: "Network unavailable" })
	await act(async () =>
		[...mount.querySelectorAll("button")].find((row) => row.textContent === "Retry")?.click(),
	)
	expect({
		rows: mount.querySelectorAll("button[data-model-option]").length,
		refreshes,
	}).toEqual({ rows: 1, refreshes: 1 })
})

test("remote refresh disables duplicate requests while retaining models", async () => {
	await renderPicker({ refreshing: true })
	const refresh = [...mount.querySelectorAll("button")].find(
		(row) => row.textContent === "Updating…",
	)
	expect({
		disabled: refresh?.disabled,
		rows: mount.querySelectorAll("button[data-model-option]").length,
	}).toEqual({ disabled: true, rows: 1 })
})
