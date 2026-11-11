import { expect, test } from "bun:test"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act } from "react"
import { desktopFoldersAtom } from "../../atoms/desktop-folders"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { createRoot } = await import("react-dom/client")
const { RuleSettings } = await import("./rule-settings")

test("browser mode does not offer desktop-only creation even with successful cached data", async () => {
	const store = createStore()
	store.set(desktopFoldersAtom, [{ id: "project", directory: "/project", addedAt: 0 }])
	const query = new QueryClient()
	query.setQueryData(["rule-files", ["/project"]], [])
	const mount = document.createElement("div")
	document.body.append(mount)
	const root = createRoot(mount)
	try {
		await act(async () =>
			root.render(
				<Provider store={store}>
					<QueryClientProvider client={query}>
						<RuleSettings embedded />
					</QueryClientProvider>
				</Provider>,
			),
		)
		expect(mount.querySelectorAll("button")).toHaveLength(0)
	} finally {
		await act(async () => root.unmount())
		mount.remove()
		query.clear()
	}
})
