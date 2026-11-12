import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { Window } from "happy-dom"
import { createStore, Provider } from "jotai"
import { act, StrictMode } from "react"
import type { Root } from "react-dom/client"
import type { WindowChromeTier } from "../../preload/api"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	localStorage: dom.localStorage,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const { chromeTierAtom, opaqueWindowsAtom } = await import("../atoms/preferences")
const { useChromeTier } = await import("./use-chrome-tier")
const { useSystemAccentColor } = await import("./use-system-accent-color")
const { createRoot } = await import("react-dom/client")

function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (error: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
let root: Root
let store: ReturnType<typeof createStore>
let colors: ReturnType<typeof deferred<string | null>>[]
let tiers: ReturnType<typeof deferred<WindowChromeTier>>[]
let colorListeners: Set<(color: string) => void>
let tierListeners: Set<(tier: WindowChromeTier) => void>
let calls: string[]
function Probe() {
	useSystemAccentColor()
	useChromeTier()
	return null
}
function tree(strict = false) {
	const child = <Provider store={store}><Probe /></Provider>
	return strict ? <StrictMode>{child}</StrictMode> : child
}
function view() {
	return {
		accent: document.documentElement.style.getPropertyValue("--system-accent"),
		tier: store.get(chromeTierAtom),
		classes: document.documentElement.className,
	}
}
beforeEach(() => {
	store = createStore()
	store.set(opaqueWindowsAtom, false)
	colors = []
	tiers = []
	colorListeners = new Set()
	tierListeners = new Set()
	calls = []
	document.documentElement.removeAttribute("style")
	document.documentElement.className = ""
	Object.assign(window, { devo: {
		platform: "darwin",
		getAccentColor: () => {
			calls.push("color:snapshot")
			const value = deferred<string | null>()
			colors.push(value)
			return value.promise
		},
		getChromeTier: () => {
			calls.push("tier:snapshot")
			const value = deferred<WindowChromeTier>()
			tiers.push(value)
			return value.promise
		},
		onAccentColorChanged: (listener: (color: string) => void) => {
			calls.push("color:subscribe")
			colorListeners.add(listener)
			return () => colorListeners.delete(listener)
		},
		onChromeTier: (listener: (tier: WindowChromeTier) => void) => {
			calls.push("tier:subscribe")
			tierListeners.add(listener)
			return () => tierListeners.delete(listener)
		},
	} })
	root = createRoot(document.createElement("div"))
})
afterEach(async () => { await act(async () => root.unmount()) })

describe("native appearance snapshots", () => {
	test("subscribe before both initial reads", async () => {
		await act(async () => root.render(tree()))
		expect(calls).toEqual(["color:subscribe", "color:snapshot", "tier:subscribe", "tier:snapshot"])
	})
	test("initial snapshot applies when there is no newer push", async () => {
		await act(async () => root.render(tree()))
		await act(async () => { colors[0].resolve("112233ff"); tiers[0].resolve("vibrancy") })
		expect(view()).toEqual({ accent: "#112233", tier: "vibrancy", classes: "electron-vibrancy" })
	})
	test("late snapshots cannot roll back live color or chrome tier", async () => {
		await act(async () => root.render(tree()))
		await act(async () => {
			for (const listener of colorListeners) listener("aabbccff")
			for (const listener of tierListeners) listener("transparent")
		})
		await act(async () => { colors[0].resolve("112233ff"); tiers[0].resolve("opaque") })
		expect(view()).toEqual({ accent: "#aabbcc", tier: "transparent", classes: "electron-transparent" })
	})
	test("StrictMode's disposed snapshots cannot replace the new effect", async () => {
		await act(async () => root.render(tree(true)))
		expect([colors.length, tiers.length, colorListeners.size, tierListeners.size]).toEqual([2, 2, 1, 1])
		await act(async () => { colors[1].resolve("aabbccff"); tiers[1].resolve("vibrancy") })
		await act(async () => { colors[0].resolve("112233ff"); tiers[0].resolve("opaque") })
		expect(view()).toEqual({ accent: "#aabbcc", tier: "vibrancy", classes: "electron-vibrancy" })
	})
	test("late reads and already queued callbacks cannot mutate after cleanup", async () => {
		await act(async () => root.render(tree()))
		const staleColor = [...colorListeners][0]
		const staleTier = [...tierListeners][0]
		await act(async () => root.render(null))
		const before = view()
		await act(async () => {
			colors[0].resolve("aabbccff")
			tiers[0].resolve("transparent")
			staleColor("112233ff")
			staleTier("vibrancy")
		})
		expect(view()).toEqual(before)
		expect([colorListeners.size, tierListeners.size]).toEqual([0, 0])
	})
	test("failed initial reads preserve fallback and live subscriptions", async () => {
		await act(async () => root.render(tree()))
		await act(async () => {
			colors[0].reject(new Error("bridge unavailable"))
			tiers[0].reject(new Error("bridge unavailable"))
		})
		expect(view()).toEqual({ accent: "", tier: "opaque", classes: "electron-opaque" })
		await act(async () => {
			for (const listener of colorListeners) listener("aabbccff")
			for (const listener of tierListeners) listener("liquid-glass")
		})
		expect(view()).toEqual({ accent: "#aabbcc", tier: "liquid-glass", classes: "electron-transparent" })
	})
	test("browser mode does not call native APIs", async () => {
		Reflect.deleteProperty(window, "devo")
		await act(async () => root.render(tree()))
		expect(calls).toEqual([])
		expect(view()).toEqual({ accent: "", tier: "opaque", classes: "" })
	})
})
