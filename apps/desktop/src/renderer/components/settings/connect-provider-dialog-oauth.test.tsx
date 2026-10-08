import { afterEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act, type ReactNode } from "react"
import type { Root } from "react-dom/client"
import type { CatalogProvider } from "../../hooks/use-devo-data"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	Node: dom.Node,
	Element: dom.Element,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})

let onDialogClose: (open: boolean) => void = () => {}
const wrap = ({ children }: { children?: ReactNode }) => <div>{children}</div>
mock.module("@devo/ui/components/dialog", () => ({
	Dialog: ({
		children,
		onOpenChange,
	}: {
		children: ReactNode
		onOpenChange: (open: boolean) => void
	}) => {
		onDialogClose = onOpenChange
		return <div>{children}</div>
	},
	DialogContent: wrap,
	DialogDescription: wrap,
	DialogFooter: wrap,
	DialogHeader: wrap,
	DialogTitle: wrap,
}))
mock.module("./provider-icon", () => ({ ProviderIcon: () => null }))
mock.module("../../services/connection-manager", () => ({
	getBaseClient: () => null,
}))
mock.module("./provider-api-key-connect", () => ({
	connectProviderWithApiKey: () => {},
}))

let update:
	| ((message: {
			instructions: string
			url?: string
			phase?: "saving"
	  }) => void)
	| undefined
let settleLogin: (() => void) | undefined
let canCancel = true
let closes = 0
let loginStarts = 0
const provider = {
	id: "openai-codex",
	name: "OpenAI",
	env: [],
	models: {},
	wireApis: [],
	enabled: true,
} satisfies CatalogProvider
Object.assign(dom, {
	devo: {
		providerOAuth: {
			login: () =>
				new Promise<void>((resolve) => {
					loginStarts++
					settleLogin = resolve
				}),
			cancel: async () => canCancel,
			onUpdate: (callback: typeof update) => {
				update = callback
				return () => {
					update = undefined
				}
			},
		},
	},
})
const { createRoot } = await import("react-dom/client")
const { ConnectProviderDialog } = await import("./connect-provider-dialog")
let root: Root | undefined
let mount: HTMLElement | undefined

async function openDialog(): Promise<HTMLElement> {
	mount = document.createElement("div")
	document.body.append(mount)
	root = createRoot(mount)
	await act(async () => {
		root?.render(
			<ConnectProviderDialog
				provider={provider}
				onClose={() => {
					closes++
				}}
				onConnected={() => {}}
			/>,
		)
	})
	return mount
}

afterEach(async () => {
	if (root)
		await act(async () => {
			root?.unmount()
		})
	root = undefined
	mount?.remove()
	mount = undefined
	closes = 0
	loginStarts = 0
	canCancel = true
	update = undefined
	settleLogin = undefined
})

describe("Desktop OAuth dialog save phase", () => {
	test("catalog refetches preserve authorization and completed connection", async () => {
		const dialog = await openDialog()
		await act(async () => {
			update?.({ instructions: "Waiting for browser login" })
			root?.render(<ConnectProviderDialog provider={{ ...provider }} onClose={() => {}} onConnected={() => {}} />)
		})
		expect({ starts: loginStarts, waiting: dialog.textContent?.includes("Waiting for browser login") }).toEqual({ starts: 1, waiting: true })
		await act(async () => { settleLogin?.() })
		await act(async () => {
			root?.render(<ConnectProviderDialog provider={{ ...provider }} onClose={() => {}} onConnected={() => {}} />)
		})
		expect({ starts: loginStarts, connected: dialog.textContent?.includes("Connected to OpenAI") }).toEqual({ starts: 1, connected: true })
	})
	test("refused Escape keeps Saving visible until credential RPC succeeds", async () => {
		const dialog = await openDialog()
		await act(async () => {
			update?.({ instructions: "Saving credential...", phase: "saving" })
		})
		expect(dialog.textContent).toContain("Saving credential... Please wait.")
		canCancel = false
		await act(async () => {
			onDialogClose(false)
		})
		expect(closes).toBe(0)
		expect(dialog.textContent).toContain("Saving credential... Please wait.")
		await act(async () => {
			settleLogin?.()
		})
		expect(dialog.textContent).toContain("Connected to OpenAI")
	})

	test("accepted Cancel closes while authorization is still pending", async () => {
		const dialog = await openDialog()
		await act(async () => {
			update?.({ instructions: "Waiting", url: "https://example.test" })
		})
		const cancel = [...dialog.querySelectorAll("button")].find(
			(button) => button.textContent === "Cancel",
		)
		expect(cancel).toBeDefined()
		await act(async () => {
			cancel?.click()
		})
		expect(closes).toBe(1)
	})
})
