import { afterEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { Provider } from "jotai"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"
import { itemsFamily } from "../atoms/messages"
import { sessionFamily } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import type { Agent } from "../lib/types"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	localStorage: dom.localStorage,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/command", () => ({
	CommandDialog: wrap,
	CommandEmpty: wrap,
	CommandGroup: wrap,
	CommandList: wrap,
	CommandSeparator: () => null,
	CommandShortcut: ({ children }: { children: ReactNode }) => <kbd>{children}</kbd>,
	CommandInput: () => <input />,
	CommandItem: ({
		children,
		disabled,
		title,
		onSelect,
	}: {
		children: ReactNode
		disabled?: boolean
		title?: string
		onSelect?: () => void
	}) => (
		<button type="button" disabled={disabled} title={title} onClick={onSelect}>
			{children}
		</button>
	),
}))
mock.module("@tanstack/react-router", () => ({
	useNavigate: () => () => {},
	useParams: () => ({ sessionId: "session", projectSlug: "project" }),
}))
mock.module("../hooks/use-agents", () => ({ useProjectList: () => [] }))
mock.module("../hooks/use-devo-data", () => ({ useServerCommands: () => [] }))
mock.module("../hooks/use-theme", () => ({
	useAvailableThemes: () => [],
	useCurrentTheme: () => ({ id: "default" }),
	useColorScheme: () => "system",
	useSetTheme: () => () => {},
	useSetColorScheme: () => () => {},
}))
const rpc = mock(() => {})
mock.module("../services/connection-manager", () => ({
	getProjectClient: () => ({ session: { revert: rpc, unrevert: rpc, abort: rpc } }),
	getBaseClient: () => null,
	reloadConfig: async () => {},
}))
const { CommandPalette } = await import("./command-palette")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
const closed = mock(() => {})
const agent: Agent = {
	id: "session",
	sessionId: "session",
	name: "Session",
	status: "idle",
	environment: "local",
	project: "Project",
	projectSlug: "project",
	directory: "/fixture",
	projectDirectory: "/fixture",
	branch: "main",
	duration: "now",
	createdAt: 1,
	lastActiveAt: 1,
	activities: [],
	permissions: [],
	questions: [],
}
async function render(agents: Agent[]) {
	const mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () =>
		root?.render(
			<Provider store={appStore}>
				<CommandPalette open onOpenChange={closed} agents={agents} />
			</Provider>,
		),
	)
	return mount
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	document.body.replaceChildren()
	appStore.set(itemsFamily("session"), [])
	appStore.set(sessionFamily("session"), null)
	rpc.mockClear()
	closed.mockClear()
})
test("palette explains unsupported session history without selectable actions or advertised shortcuts", async () => {
	appStore.set(sessionFamily("session"), {
		session: { id: "session" },
		status: { type: "idle" },
		directory: "/fixture",
		permissions: [],
		questions: [],
	})
	appStore.set(itemsFamily("session"), [
		{
			id: "item",
			sessionId: "session",
			turnId: "turn",
			seq: 1,
			revision: 0,
			createdAt: "2026-11-09T00:00:00Z",
			updatedAt: "2026-11-09T00:00:00Z",
			state: "completed",
			item: { type: "userMessage", content: [{ type: "text", text: "Original" }] },
		},
	])
	const mount = await render([agent])
	const rows = [...mount.querySelectorAll("button")].filter((button) =>
		/^(Undo Last Turn|Redo)/.test(button.textContent ?? ""),
	)
	expect(
		rows.map((row) => ({
			disabled: row.disabled,
			unavailable: row.textContent?.includes("Not supported"),
			shortcut: row.querySelector("kbd")?.textContent ?? null,
		})),
	).toEqual([
		{ disabled: true, unavailable: true, shortcut: null },
		{ disabled: true, unavailable: true, shortcut: null },
	])
	for (const row of rows) {
		expect(row.title).toContain("not supported")
		await act(async () => row.click())
	}
	expect({ rpc: rpc.mock.calls, closed: closed.mock.calls }).toEqual({ rpc: [], closed: [] })
})
test("palette does not show session-history entries without an active session", async () => {
	const mount = await render([])
	expect(mount.textContent).not.toContain("Undo Last Turn")
	expect(mount.textContent).not.toContain("Redo")
})
