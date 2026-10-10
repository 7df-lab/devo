import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"
import type {
	CatalogProviderInfo,
	ProviderCatalogData,
} from "../../../hooks/use-devo-data"

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
const provider = (id: string, name: string): CatalogProviderInfo => ({
	id,
	name,
	enabled: true,
	wireApis: [],
	models: { "remote-model": { name: "Remote model" } },
})
let data: ProviderCatalogData
let serverConnected = true
let catalogError: string | null = null
let offline = false
const refreshed: string[] = []
const completed: number[] = []
let skipped = 0
let reloaded = 0
let invalidated = 0
let oauthProps: any
let templateProps: any
const refreshCatalog = (policy: string) => {
	refreshed.push(policy)
}
const reload = () => {
	reloaded++
}
mock.module("../../../hooks/use-server", () => ({
	useServerConnection: () => ({ connected: serverConnected }),
}))
mock.module("../../../hooks/use-devo-data", () => ({
	useProviderCatalog: () => ({
		data,
		loading: false,
		error: catalogError,
		reload,
	}),
}))
mock.module("../../../hooks/use-model-directory", () => ({
	useModelDirectory: () => ({
		refreshCatalog,
		refreshing: false,
		error: null,
		offline,
	}),
}))
mock.module("../../../lib/invalidate-provider-queries", () => ({
	invalidateProviderDependentQueries: () => {
		invalidated++
	},
}))
mock.module("@devo/ui/components/input", () => ({
	Input: ({ onChange, ...props }: any) => (
		<input {...props} onInput={onChange} />
	),
}))
mock.module("../../settings/provider-icon", () => ({
	ProviderIcon: () => null,
}))
mock.module("../../settings/connect-provider-dialog", () => ({
	ConnectProviderDialog: (props: any) => {
		oauthProps = props
		return <div>OAuth connection</div>
	},
}))
mock.module("../../settings/template-connect-dialog", () => ({
	TemplateConnectDialog: (props: any) => {
		templateProps = props
		return <div>API connection</div>
	},
}))
mock.module("../../settings/connection-detail-dialog", () => ({
	ConnectionDetailDialog: (props: any) => (
		<div>Manage {props.provider.name}</div>
	),
}))
const { ProviderSetupStep } = await import("./provider-setup-step")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let mount: HTMLElement
beforeEach(() => {
	data = {
		providers: [
			provider("groq", "Groq"),
			provider("new-provider", "Remote provider"),
			provider("openai-codex", "ChatGPT"),
			provider("anthropic", "Anthropic"),
		],
		templateIds: new Set(["groq", "new-provider", "openai-codex", "anthropic"]),
		connectedIds: new Set(["openai-codex"]),
		connectionModels: {},
	}
	serverConnected = true
	catalogError = null
	offline = false
	refreshed.length = 0
	completed.length = 0
	skipped = 0
	reloaded = 0
	invalidated = 0
	oauthProps = null
	templateProps = null
})
afterEach(async () => {
	await act(async () => root?.unmount())
	mount?.remove()
	root = undefined
})
async function render() {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root!.render(
			<ProviderSetupStep
				onComplete={(count) => completed.push(count)}
				onSkip={() => {
					skipped++
				}}
			/>,
		),
	)
}
async function click(text: string) {
	const button = [...mount.querySelectorAll("button")].find((button) =>
		button.textContent?.includes(text),
	)
	if (!button) throw new Error(`Missing button ${text}`)
	await act(async () => button.click())
}

test("onboarding shows the complete server catalog and requests a stale remote refresh", async () => {
	await render()
	expect(
		[...mount.querySelectorAll('[aria-label="Provider catalog"] button')].map(
			(button) => button.textContent,
		),
	).toEqual([
		"ChatGPT1 model · Connected",
		"Anthropic1 model",
		"Groq1 model",
		"Remote provider1 model",
	])
	expect(refreshed).toEqual(["ifStale"])
	data = {
		...data,
		providers: [...data.providers, provider("remote-added", "Added remotely")],
	}
	await render()
	expect({
		added: mount.textContent?.includes("Added remotely"),
		refreshes: refreshed,
	}).toEqual({ added: true, refreshes: ["ifStale"] })
})

test("search finds remote model names, handles no results, and refresh uses the canonical path", async () => {
	data.providers[1].models = { "new-remote-id": { name: "Nova remote" } }
	await render()
	const input = mount.querySelector("input")!
	await act(async () => {
		input.value = "Nova"
		input.dispatchEvent(
			new dom.Event("input", { bubbles: true }) as unknown as Event,
		)
	})
	expect(
		[...mount.querySelectorAll('[aria-label="Provider catalog"] button')].map(
			(button) => button.textContent,
		),
	).toEqual(["Remote provider1 model"])
	await act(async () => {
		input.value = "missing"
		input.dispatchEvent(
			new dom.Event("input", { bubbles: true }) as unknown as Event,
		)
	})
	expect(mount.textContent).toContain(
		"No providers or models match your search.",
	)
	await click("Refresh catalog")
	expect({ refreshes: refreshed, reloads: reloaded }).toEqual({
		refreshes: ["ifStale", "force"],
		reloads: 1,
	})
})

test("onboarding reuses API and OAuth dialogs and catalog refetches preserve OAuth state", async () => {
	await render()
	await click("Groq")
	expect(templateProps.provider).toEqual(data.providers[0])
	await act(async () => templateProps.onConnected())
	expect({ reloads: reloaded, invalidations: invalidated }).toEqual({
		reloads: 1,
		invalidations: 1,
	})
	await click("Anthropic")
	const original = oauthProps.provider
	data = { ...data, providers: data.providers.map((entry) => ({ ...entry })) }
	await render()
	expect(oauthProps.provider).toBe(original)
	await act(async () => oauthProps.onClose())
	await click("ChatGPT")
	expect(mount.textContent).toContain("Manage ChatGPT")
})

test("offline and failed refresh keep saved connections usable and allow continuing", async () => {
	offline = true
	catalogError = "Network unavailable"
	await render()
	expect({
		error: mount
			.querySelector('[role="alert"]')
			?.textContent?.includes("Network unavailable"),
		offline: mount.querySelector('[role="status"]')?.textContent,
	}).toEqual({
		error: true,
		offline: "Offline mode · showing the saved catalog.",
	})
	await click("Continue")
	expect(completed).toEqual([1])
	data = { ...data, connectedIds: new Set() }
	await render()
	await click("I'll do this later")
	expect(skipped).toBe(1)
})

test("waiting for the server does not start a refresh and permits skipping", async () => {
	serverConnected = false
	await render()
	await click("Skip for now")
	expect({ refreshes: refreshed, skipped }).toEqual({
		refreshes: [],
		skipped: 1,
	})
})
