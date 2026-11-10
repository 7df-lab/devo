import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider, useAtomValue } from "jotai"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { Automation } from "../../../preload/api"
import { automationsAtom, setAutomationsAtom } from "../../atoms/automations"

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
	ShadowRoot: dom.ShadowRoot,
	Event: dom.Event,
	MouseEvent: dom.MouseEvent,
	MutationObserver: dom.MutationObserver,
	ResizeObserver: dom.ResizeObserver,
	getComputedStyle: dom.getComputedStyle.bind(dom),
	requestAnimationFrame: dom.requestAnimationFrame.bind(dom),
	cancelAnimationFrame: dom.cancelAnimationFrame.bind(dom),
	IS_REACT_ACT_ENVIRONMENT: true,
})
const update = mock(async (_input: unknown) => null)
const changed = mock((_open: boolean) => {})
const providers = { providers: [] }
mock.module("../../hooks/use-devo-data", () => ({
	useProviders: () => ({ data: providers }),
	useDevoAgents: () => ({ agents: [{ name: "build" }] }),
	getModelVariants: () => ["low", "high"],
}))
mock.module("../../services/backend", () => ({
	updateAutomation: update,
	createAutomation: mock(async () => null),
	deleteAutomation: mock(async () => {}),
	runAutomationNow: mock(async () => {}),
	pickDirectory: mock(async () => "/draft-project"),
}))
// Exercise the real dialog; isolate the independent picker interfaces.
mock.module("./schedule-picker", () => ({
	SchedulePicker: ({ value, onChange }: { value: string; onChange: (value: string) => void }) => (
		<input aria-label="Schedule" value={value} onChange={(event) => onChange(event.target.value)} />
	),
}))
mock.module("../chat/prompt-toolbar", () => ({
	AgentSelector: ({
		selectedAgent,
		onSelectAgent,
	}: {
		selectedAgent: string | null
		onSelectAgent: (value: string) => void
	}) => (
		<input
			aria-label="Agent"
			value={selectedAgent ?? ""}
			onChange={(event) => onSelectAgent(event.target.value)}
		/>
	),
	ModelSelector: ({
		effectiveModel,
		onSelectModel,
	}: {
		effectiveModel: { modelID: string } | null
		onSelectModel: (value: { providerID: string; modelID: string }) => void
	}) => (
		<input
			aria-label="Model"
			value={effectiveModel?.modelID ?? ""}
			onChange={(event) => onSelectModel({ providerID: "provider", modelID: event.target.value })}
		/>
	),
	VariantSelector: ({
		selectedVariant,
		onSelectVariant,
	}: {
		selectedVariant?: string
		onSelectVariant: (value: string) => void
	}) => (
		<input
			aria-label="Variant"
			value={selectedVariant ?? ""}
			onChange={(event) => onSelectVariant(event.target.value)}
		/>
	),
}))
const { createRoot } = await import("react-dom/client")
const { CreateAutomationDialog } = await import("./create-automation-dialog")
const original: Automation = {
	id: "first",
	name: "Saved name",
	prompt: "Saved prompt",
	workspaces: ["/saved-project"],
	schedule: { rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0", timezone: "UTC" },
	execution: {
		effort: "medium",
		model: "provider/saved-model",
		agent: "build",
		variant: "low",
		timeout: 300,
		retries: 0,
		retryDelay: 30,
		parallelWorkspaces: false,
		approvalPolicy: "never",
		useWorktree: true,
		permissionPreset: "default",
	},
	status: "active",
	createdAt: 1,
	runCount: 0,
	nextRunAt: null,
	lastRunAt: null,
	consecutiveFailures: 0,
	updatedAt: 1,
}
let root: Root | undefined
let mount: HTMLElement
let store = createStore()
function Harness({ open, target }: { open: boolean; target: string | null }) {
	const automations = useAtomValue(automationsAtom)
	return (
		<CreateAutomationDialog
			open={open}
			onOpenChange={changed}
			editAutomation={automations.find((automation) => automation.id === target) ?? null}
		/>
	)
}
async function render(open = true, target: string | null = "first") {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
		store.set(setAutomationsAtom, structuredClone([original]))
	}
	await act(async () =>
		root?.render(
			<StrictMode>
				<Provider store={store}>
					<Harness open={open} target={target} />
				</Provider>
			</StrictMode>,
		),
	)
}
async function fill(selector: string, value: string) {
	const input = document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement
	expect(input).not.toBeNull()
	await act(async () => {
		const prototype =
			input.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
		Object.getOwnPropertyDescriptor(prototype, "value")?.set?.call(input, value)
		input.dispatchEvent(new Event("input", { bubbles: true }))
	})
}
function value(selector: string) {
	return (document.querySelector(selector) as HTMLInputElement).value
}
async function click(label: string) {
	const button = [...document.querySelectorAll("button")].find(
		(entry) => entry.textContent?.trim() === label,
	)
	expect(button).toBeDefined()
	await act(async () => button?.click())
}
async function refresh(automations: Automation[]) {
	// Same atom write used by useAutomationData after IPC polls/push refreshes.
	await act(async () => {
		store.set(setAutomationsAtom, structuredClone(automations))
	})
}
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	root = undefined
	store = createStore()
	update.mockClear()
	changed.mockClear()
})

test("same-ID polling refresh preserves every draft field in StrictMode", async () => {
	await render()
	await fill("#automation-name", "Draft name")
	await fill("#automation-prompt", "Draft prompt")
	await fill('[aria-label="Schedule"]', "FREQ=HOURLY;INTERVAL=3")
	await fill('[aria-label="Model"]', "draft-model")
	await fill('[aria-label="Agent"]', "plan")
	await fill('[aria-label="Variant"]', "high")
	await click("Browse for folder")
	await refresh([{ ...original, runCount: 1 }])
	expect(value("#automation-name")).toBe("Draft name")
	expect(value("#automation-prompt")).toBe("Draft prompt")
	expect(value('[aria-label="Schedule"]')).toBe("FREQ=HOURLY;INTERVAL=3")
	expect(value('[aria-label="Model"]')).toBe("draft-model")
	expect(value('[aria-label="Agent"]')).toBe("plan")
	expect(value('[aria-label="Variant"]')).toBe("high")
	expect(document.querySelector('[aria-label="Remove draft-project"]')).not.toBeNull()
	expect(document.body.textContent).toContain("1 runs")
	await click("Save")
	expect(update).toHaveBeenCalledWith({
		id: "first",
		name: "Draft name",
		prompt: "Draft prompt",
		schedule: {
			rrule: "FREQ=HOURLY;INTERVAL=3",
			timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
		},
		workspaces: ["/saved-project", "/draft-project"],
		execution: { effort: "medium", model: "provider/draft-model", agent: "plan", variant: "high" },
	})
})

test("switching target while open initializes the new automation", async () => {
	await render()
	await fill("#automation-name", "Discarded draft")
	await refresh([
		original,
		{
			...original,
			id: "second",
			name: "Second automation",
			prompt: "Second prompt",
			execution: { ...original.execution, model: undefined, agent: undefined, variant: undefined },
		},
	])
	await render(true, "second")
	expect(value("#automation-name")).toBe("Second automation")
	expect(value("#automation-prompt")).toBe("Second prompt")
	expect(value('[aria-label="Model"]')).toBe("")
	expect(value('[aria-label="Agent"]')).toBe("")
})

test("close and reopen loads latest saved data instead of the old draft", async () => {
	await render(false)
	await render()
	await fill("#automation-name", "Discarded draft")
	await render(false)
	await refresh([{ ...original, name: "Latest saved name" }])
	await render()
	expect(value("#automation-name")).toBe("Latest saved name")
})

test("create starts empty after editing and resets on reopen", async () => {
	await render()
	await render(false)
	await render(true, null)
	expect(value("#automation-name")).toBe("")
	expect(value("#automation-prompt")).toBe("")
	expect(value('[aria-label="Model"]')).toBe("")
	expect(document.querySelector('[aria-label="Remove saved-project"]')).toBeNull()
	await fill("#automation-name", "New draft")
	await fill("#automation-prompt", "New prompt")
	await refresh([{ ...original, runCount: 2 }])
	expect(value("#automation-name")).toBe("New draft")
	await render(false, null)
	await render(true, null)
	expect(value("#automation-name")).toBe("")
	expect(value("#automation-prompt")).toBe("")
})
