import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import { Window } from "happy-dom"
import { Provider, useAtomValue } from "jotai"
import { act, Suspense, startTransition } from "react"
import type { Root } from "react-dom/client"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import {
	removeSessionAtom,
	sessionFamily,
	sessionIdsAtom,
	upsertSessionAtom,
} from "../atoms/sessions"
import { appStore } from "../atoms/store"
import type { Session } from "../lib/types"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	Event: dom.Event,
	IS_REACT_ACT_ENVIRONMENT: true,
})
const calls: Array<{ id: string; server: string | null }> = []
let read: (id: string) => Promise<Session | null> = async () => null
mock.module("../services/connection-manager", () => ({
	fetchSessionById: (id: string) => {
		calls.push({ id, server: appStore.get(serverUrlAtom) })
		return read(id)
	},
}))
const { useSessionResolution } = await import("./use-session-resolution")
const { SessionResolutionState } = await import("../components/session-resolution-state")
const { createRoot } = await import("react-dom/client")
let root: Root | undefined
let mount: HTMLElement
let result: ReturnType<typeof useSessionResolution>
function child(id = "child"): Session {
	return {
		id,
		title: "Saved child",
		parentId: "parent",
		directory: "/repo/child",
		time: { created: 1, updated: 1 },
	}
}
function deferred() {
	let resolve!: (session: Session | null) => void
	let reject!: (error: Error) => void
	const promise = new Promise<Session | null>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
const suspendedNavigation = new Promise<void>(() => {})
function Harness({ id, active, suspend }: { id: string; active: boolean; suspend?: boolean }) {
	const session = useAtomValue(sessionFamily(id))
	result = useSessionResolution({ sessionId: id, active, known: Boolean(session) })
	if (suspend) throw suspendedNavigation
	return <SessionResolutionState state={result.state} onRetry={result.retry} />
}
async function render(id = "child", active = true) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () =>
		root?.render(
			<Provider store={appStore}>
				<Suspense fallback={<span>Navigation pending</span>}>
					<Harness id={id} active={active} />
				</Suspense>
			</Provider>,
		),
	)
}
beforeEach(() => {
	appStore.set(serverConnectedAtom, true)
	appStore.set(serverUrlAtom, "runtime-one")
	appStore.set(activeServerIdAtom, "one")
})
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	for (const id of appStore.get(sessionIdsAtom)) appStore.set(removeSessionAtom, id)
	calls.length = 0
	read = async () => null
})

describe("cold child session resolution", () => {
	test("offline is distinct from missing and reconnect retries the exact saved child", async () => {
		appStore.set(serverConnectedAtom, false)
		const pending = deferred()
		read = () => pending.promise
		await render()
		expect(calls).toEqual([])
		expect(result.state).toEqual({ status: "offline" })
		expect(mount.textContent).toContain("Runtime disconnected")
		expect(mount.textContent).not.toContain("Session not found")
		await act(async () => appStore.set(serverConnectedAtom, true))
		expect(result.state).toEqual({ status: "loading" })
		expect(mount.querySelector('output[aria-label="Loading session"]')).not.toBeNull()
		expect(calls).toEqual([{ id: "child", server: "runtime-one" }])
		await act(async () => pending.resolve(child()))
		expect(appStore.get(sessionFamily("child"))?.session).toEqual(child())
		expect(result.state).toEqual({ status: "idle" })
	})

	test("a failed read shows an actionable error; repeated retry shares one pending request", async () => {
		read = async () => {
			throw new Error("Connection timed out")
		}
		await render()
		expect(result.state).toEqual({ status: "error", message: "Connection timed out" })
		expect(mount.querySelector('[role="alert"]')?.textContent).toBe("Could not open session")
		expect(mount.textContent).toContain("Connection timed out")
		expect(mount.textContent).not.toContain("Session not found")
		const pending = deferred()
		read = () => pending.promise
		await act(async () => {
			mount.querySelector<HTMLButtonElement>('button[aria-label="Retry opening session"]')?.click()
			result.retry()
			result.retry()
		})
		expect(calls).toHaveLength(2)
		expect(mount.querySelector("button")).toBeNull()
		await act(async () => pending.resolve(child()))
		expect(result.state).toEqual({ status: "idle" })
	})

	test("only a confirmed missing result shows not found and allows a fresh explicit retry", async () => {
		await render()
		expect(result.state).toEqual({ status: "notFound" })
		expect(mount.textContent).toContain("Session not found")
		expect(mount.textContent).toContain("The runtime could not find this session")
		read = async () => child()
		await act(async () => result.retry())
		expect(calls).toHaveLength(2)
		expect(result.state).toEqual({ status: "idle" })
	})

	test("known, blank and inactive sessions do not trigger discovery", async () => {
		await render("")
		await render("child", false)
		await act(async () =>
			appStore.set(upsertSessionAtom, { session: child(), directory: "/repo/child" }),
		)
		await render("child")
		expect(calls).toEqual([])
		expect(result.state).toEqual({ status: "idle" })
	})

	test("navigation discards a late response instead of inserting the previous child", async () => {
		const first = deferred(),
			second = deferred()
		read = (id) => (id === "child" ? first.promise : second.promise)
		await render()
		await render("other")
		await act(async () => first.resolve(child()))
		expect(appStore.get(sessionFamily("child"))).toBeNull()
		expect(result.state).toEqual({ status: "loading" })
		await act(async () => second.resolve(child("other")))
		expect(appStore.get(sessionFamily("other"))?.session).toEqual(child("other"))
	})

	test("server switching rejects the old server's same-ID result and retries on the new client", async () => {
		const first = deferred(),
			second = deferred()
		read = () => first.promise
		await render()
		read = () => second.promise
		await act(async () => {
			appStore.set(serverUrlAtom, "runtime-two")
			appStore.set(activeServerIdAtom, "two")
			first.resolve(child())
		})
		expect(appStore.get(sessionFamily("child"))).toBeNull()
		await act(async () => second.reject(new Error("New runtime unavailable")))
		expect(result.state).toEqual({ status: "error", message: "New runtime unavailable" })
		read = async () => child()
		await act(async () => result.retry())
		expect(calls).toEqual([
			{ id: "child", server: "runtime-one" },
			{ id: "child", server: "runtime-two" },
			{ id: "child", server: "runtime-two" },
		])
	})

	test("disconnect invalidates an in-flight result; reconnect performs a fresh read", async () => {
		const pending = deferred()
		read = () => pending.promise
		await render()
		await act(async () => {
			appStore.set(serverConnectedAtom, false)
			pending.resolve(child())
		})
		expect(appStore.get(sessionFamily("child"))).toBeNull()
		expect(result.state).toEqual({ status: "offline" })
		read = async () => child()
		await act(async () => appStore.set(serverConnectedAtom, true))
		expect(calls).toHaveLength(2)
		expect(result.state).toEqual({ status: "idle" })
	})

	test("an arriving live session wins over an older discovery snapshot", async () => {
		const pending = deferred()
		read = () => pending.promise
		await render()
		const live = { ...child(), title: "Live title" }
		await act(async () => {
			appStore.set(upsertSessionAtom, { session: live, directory: "/repo/child" })
			pending.resolve(child())
		})
		expect(appStore.get(sessionFamily("child"))?.session).toEqual(live)
		expect(result.state).toEqual({ status: "idle" })
	})

	test("a different returned ID is an error and is never inserted", async () => {
		read = async () => child("wrong")
		await render()
		expect(result.state).toEqual({
			status: "error",
			message: "The runtime returned a different session.",
		})
		expect(appStore.get(sessionIdsAtom)).toEqual(new Set())
	})

	test("a suspended navigation does not invalidate the still-visible session's read", async () => {
		const pending = deferred()
		read = () => pending.promise
		await render()
		await act(async () =>
			startTransition(() =>
				root?.render(
					<Provider store={appStore}>
						<Suspense fallback={<span>Navigation pending</span>}>
							<Harness id="other" active suspend />
						</Suspense>
					</Provider>,
				),
			),
		)
		expect(calls).toEqual([{ id: "child", server: "runtime-one" }])
		await act(async () => pending.resolve(child()))
		expect(appStore.get(sessionFamily("child"))?.session).toEqual(child())
		expect(appStore.get(sessionFamily("other"))).toBeNull()
	})

	test("unmount discards a pending result", async () => {
		const pending = deferred()
		read = () => pending.promise
		await render()
		await act(async () => {
			root?.unmount()
			root = undefined
		})
		await act(async () => pending.resolve(child()))
		expect(appStore.get(sessionIdsAtom)).toEqual(new Set())
	})

	test("hiding a kept-alive panel stops discovery; returning retries without accepting the old result", async () => {
		const first = deferred(),
			second = deferred()
		read = () => first.promise
		await render()
		await render("child", false)
		read = () => second.promise
		await render()
		await act(async () => first.resolve(child()))
		expect(appStore.get(sessionIdsAtom)).toEqual(new Set())
		await act(async () => second.resolve(child()))
		expect(calls).toHaveLength(2)
		expect(result.state).toEqual({ status: "idle" })
	})
})
