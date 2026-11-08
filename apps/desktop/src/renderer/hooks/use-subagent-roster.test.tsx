import { afterEach, describe, expect, mock, test } from "bun:test"
import type { NativeSession, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { act } from "react"
import type { Root } from "react-dom/client"

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	IS_REACT_ACT_ENVIRONMENT: true,
})
let read = async (_params: unknown): Promise<SessionRosterPage> => ({
	data: [],
	nextCursor: null,
})
let connected = true
mock.module("../services/connection-manager", () => ({
	getProjectClient: () =>
		connected ? { session: { roster: (params: unknown) => read(params) } } : null,
}))
const { readSubagentRoster, useSubagentRoster } = await import("./use-subagent-roster")
const { createRoot } = await import("react-dom/client")

function child(id: string, parentId = "parent"): NativeSession {
	return {
		id,
		parent: { kind: "agent", sessionId: parentId },
		ephemeral: false,
	} as NativeSession
}
function deferred<T>() {
	let resolve!: (value: T) => void
	let reject!: (cause: Error) => void
	const promise = new Promise<T>((yes, no) => {
		resolve = yes
		reject = no
	})
	return { promise, resolve, reject }
}
let root: Root | undefined
let mount: HTMLElement
let state: ReturnType<typeof useSubagentRoster>
function Harness({ id, open }: { id: string; open: boolean }) {
	state = useSubagentRoster(id, "/repo", open)
	return <div>{state.sessions.map((session) => session.id).join(",")}</div>
}
async function render(id = "parent", open = true) {
	if (!root) {
		mount = document.createElement("div")
		document.body.append(mount)
		root = createRoot(mount)
	}
	await act(async () => {
		root?.render(<Harness id={id} open={open} />)
	})
}
afterEach(async () => {
	await act(async () => root?.unmount())
	root = undefined
	mount?.remove()
	connected = true
	read = async () => ({ data: [], nextCursor: null })
})

describe("bounded child roster scan", () => {
	test("finds children on later pages, excluding roots, forks, grandchildren and ephemeral children", async () => {
		const calls: Array<string | undefined> = []
		const result = await readSubagentRoster(
			async (cursor) => {
				calls.push(cursor)
				return cursor
					? {
							data: [child("right"), child("grandchild", "right")],
							nextCursor: null,
						}
					: {
							data: [
								{ id: "root" },
								{ id: "fork", forkFromId: "parent" },
								{ ...child("side"), ephemeral: true },
								child("other", "elsewhere"),
							] as NativeSession[],
							nextCursor: "4",
						}
			},
			"parent",
			new AbortController().signal,
		)
		expect(result).toEqual({
			sessions: [child("right")],
			nextCursor: null,
			scanned: 6,
		})
		expect(calls).toEqual([undefined, "4"])
	})
	test("caps each scan at five pages and continues explicitly without duplicates", async () => {
		const read = async (cursor?: string) => ({
			data: [child("same")],
			nextCursor: String(Number(cursor ?? 0) + 1),
		})
		const signal = new AbortController().signal
		const first = await readSubagentRoster(read, "parent", signal)
		expect(first).toEqual({
			sessions: [child("same")],
			nextCursor: "5",
			scanned: 5,
		})
		expect(await readSubagentRoster(read, "parent", signal, first)).toEqual({
			sessions: [child("same")],
			nextCursor: "10",
			scanned: 10,
		})
	})
	test("rejects repeated cursors rather than silently claiming a complete roster", async () => {
		await expect(
			readSubagentRoster(
				async () => ({ data: [], nextCursor: "same" }),
				"parent",
				new AbortController().signal,
			),
		).rejects.toThrow("repeated a session page")
	})
	test("aborting between pages prevents further reads", async () => {
		const controller = new AbortController()
		let calls = 0
		await expect(
			readSubagentRoster(
				async () => {
					calls++
					controller.abort()
					return { data: [], nextCursor: "more" }
				},
				"parent",
				controller.signal,
			),
		).rejects.toThrow()
		expect(calls).toBe(1)
	})
})

describe("roster lifecycle", () => {
	test("closed panels make no reads; repeated refreshes share the active scan", async () => {
		const pending = deferred<SessionRosterPage>()
		let calls = 0
		read = async () => {
			calls++
			return pending.promise
		}
		await render("parent", false)
		expect(calls).toBe(0)
		await render()
		await act(async () => {
			void state.refresh()
			void state.refresh()
		})
		expect(calls).toBe(1)
		await act(async () => pending.resolve({ data: [child("one")], nextCursor: null }))
		expect(state.sessions).toEqual([child("one")])
	})
	test("closing and reopening ignores an old in-flight response", async () => {
		const old = deferred<SessionRosterPage>()
		read = async () => old.promise
		await render()
		await render("parent", false)
		read = async () => ({ data: [child("new")], nextCursor: null })
		await render()
		await act(async () => old.resolve({ data: [child("old")], nextCursor: null }))
		expect(state.sessions).toEqual([child("new")])
		expect(state.loading).toBe(false)
	})
	test("navigation and unmount discard obsolete scans", async () => {
		const old = deferred<SessionRosterPage>()
		read = async () => old.promise
		await render()
		read = async () => ({ data: [child("new", "other")], nextCursor: null })
		await render("other")
		await act(async () => old.resolve({ data: [child("old")], nextCursor: null }))
		expect(state.sessions).toEqual([child("new", "other")])
		await act(async () => root?.unmount())
		root = undefined
	})
	test("refresh errors retain the previous snapshot and allow retry", async () => {
		read = async () => ({ data: [child("one")], nextCursor: null })
		await render()
		read = async () => {
			throw new Error("offline")
		}
		await act(async () => state.refresh())
		expect({
			sessions: state.sessions,
			error: state.error,
			loading: state.loading,
		}).toEqual({ sessions: [child("one")], error: "offline", loading: false })
		read = async () => ({ data: [], nextCursor: null })
		await act(async () => state.refresh())
		expect({ sessions: state.sessions, error: state.error }).toEqual({
			sessions: [],
			error: null,
		})
	})
	test("disconnection is an explicit error, not an empty success", async () => {
		connected = false
		await render()
		expect(state.error).toBe("Connect to the runtime to view subagents.")
	})
})
