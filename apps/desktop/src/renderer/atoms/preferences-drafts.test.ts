import { afterAll, expect, test } from "bun:test"
import { createStore } from "jotai"

const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage")
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window")
const savedDrafts = { "qa-session": "An unsent draft survives reload." }
const values = new Map([["devo:drafts", JSON.stringify(savedDrafts)]])

Object.defineProperty(globalThis, "localStorage", {
	configurable: true,
	value: {
		getItem: (key: string) => values.get(key) ?? null,
		setItem: (key: string, value: string) => values.set(key, value),
		removeItem: (key: string) => values.delete(key),
	},
})
Object.defineProperty(globalThis, "window", {
	configurable: true,
	value: { localStorage },
})

afterAll(() => {
	if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage)
	else Reflect.deleteProperty(globalThis, "localStorage")
	if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow)
	else Reflect.deleteProperty(globalThis, "window")
})

test("draft snapshots hydrate saved text before the composer subscribes", async () => {
	const { draftsAtom, setDraftAtom, clearDraftAtom } = await import("./preferences")
	const store = createStore()
	expect(store.get(draftsAtom)).toEqual(savedDrafts)
	store.set(setDraftAtom, { key: "qa-other-session", text: "Another draft" })
	expect(JSON.parse(values.get("devo:drafts")!)).toEqual({
		...savedDrafts,
		"qa-other-session": "Another draft",
	})
	store.set(clearDraftAtom, "qa-session")
	expect(JSON.parse(values.get("devo:drafts")!)).toEqual({
		"qa-other-session": "Another draft",
	})
})
