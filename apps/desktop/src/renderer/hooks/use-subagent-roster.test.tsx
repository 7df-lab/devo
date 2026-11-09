import { afterEach, beforeEach, describe, expect, jest, mock, test } from "bun:test"
import type { NativeSession, SessionRosterChange, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { Window } from "happy-dom"
import { getDefaultStore } from "jotai"
import { act } from "react"
import type { Root } from "react-dom/client"
import { serverConnectedAtom, serverUrlAtom } from "../atoms/connection"

const listeners = new Set<(change: SessionRosterChange) => void>()
const observeRoster = (listener: (change: SessionRosterChange) => void) => {
	listeners.add(listener)
	return () => {
		listeners.delete(listener)
	}
}
function emit(change: SessionRosterChange) {
	for (const listener of listeners) listener(change)
}
beforeEach(() => {
	getDefaultStore().set(serverConnectedAtom, true)
})

const dom = new Window({ url: "http://localhost" })
Object.assign(globalThis, {
	window: dom,
	document: dom.document,
	navigator: dom.navigator,
	HTMLElement: dom.HTMLElement,
	Event: dom.Event,
	IS_REACT_ACT_ENVIRONMENT: true,
})
let read = async (_params: unknown): Promise<SessionRosterPage> => ({
	data: [],
	nextCursor: null,
})
let connected = true
mock.module("../services/connection-manager", () => ({
	getProjectClient: () =>
		connected ? { session: { roster: (params: unknown) => read(params), observeRoster } } : null,
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
	jest.useRealTimers()
	expect(listeners.size).toBe(0)
	getDefaultStore().set(serverUrlAtom, null)
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

describe("automatic roster updates", () => {
	test("Native mutations update immediately without extra roster scans and filter unrelated children", async () => {
		let calls = 0
		read = async () => {
			calls++
			return { data: [child("one")], nextCursor: null }
		}
		await render()
		await act(async () => {
			emit({
				type: "patch",
				sessionId: "one",
				patch: { status: "active", flags: ["waitingApproval"] },
			})
			emit({ type: "upsert", session: child("new") })
			emit({ type: "upsert", session: child("other", "elsewhere") })
			emit({ type: "upsert", session: { ...child("side"), ephemeral: true } })
		})
		expect(state.sessions).toEqual([
			{ ...child("one"), status: "active", flags: ["waitingApproval"] },
			child("new"),
		])
		await act(async () => {
			emit({
				type: "patch",
				sessionId: "one",
				patch: { queuedCount: 2, archived: true, cwd: "/new" },
			})
			emit({ type: "delete", sessionIds: ["new"] })
		})
		expect(state.sessions).toEqual([
			{
				...child("one"),
				status: "active",
				flags: ["waitingApproval"],
				queuedCount: 2,
				archived: true,
				cwd: "/new",
			},
		])
		expect(calls).toBe(1)
	})

	test("events during a scan override old pages, including unknown children and deletion tombstones", async () => {
		const pending = deferred<SessionRosterPage>()
		read = async () => pending.promise
		await render()
		await act(async () => {
			emit({ type: "patch", sessionId: "one", patch: { status: "active" } })
			emit({ type: "patch", sessionId: "one", patch: { flags: ["waitingUserInput"] } })
			emit({ type: "delete", sessionIds: ["gone"] })
			emit({ type: "patch", sessionId: "gone", patch: { status: "active" } })
			emit({ type: "upsert", session: child("new") })
			emit({ type: "patch", sessionId: "new", patch: { queuedCount: 3 } })
			pending.resolve({ data: [child("one"), child("gone")], nextCursor: null })
		})
		expect(state.sessions).toEqual([
			{ ...child("one"), status: "active", flags: ["waitingUserInput"] },
			{ ...child("new"), queuedCount: 3 },
		])
	})

	test("a moved or ephemeral child's metadata removes the old row even during a scan", async () => {
		read = async () => ({ data: [child("one")], nextCursor: null })
		await render()
		const pending = deferred<SessionRosterPage>()
		read = async () => pending.promise
		await act(async () => {
			void state.refresh()
		})
		await act(async () => {
			emit({ type: "upsert", session: child("one", "elsewhere") })
			pending.resolve({ data: [child("one")], nextCursor: null })
		})
		expect(state.sessions).toEqual([])
	})

	test("reconciles missed events every 30 seconds only while open; manual refresh never overlaps", async () => {
		jest.useFakeTimers()
		let calls = 0
		read = async () => {
			calls++
			return { data: [child(String(calls))], nextCursor: null }
		}
		await render()
		await act(async () => {
			jest.advanceTimersByTime(29_999)
		})
		expect(calls).toBe(1)
		await act(async () => {
			jest.advanceTimersByTime(1)
		})
		expect(state.sessions).toEqual([child("2")])
		const pending = deferred<SessionRosterPage>()
		read = async () => {
			calls++
			return pending.promise
		}
		await act(async () => {
			void state.refresh()
			void state.refresh()
		})
		await act(async () => {
			jest.advanceTimersByTime(90_000)
		})
		expect(calls).toBe(3)
		await render("parent", false)
		await act(async () => {
			pending.resolve({ data: [child("late")], nextCursor: null })
			jest.advanceTimersByTime(90_000)
		})
		expect(calls).toBe(3)
		expect(state.sessions).toEqual([])
		expect(listeners.size).toBe(0)
	})

	test("disconnect preserves rows, stops reconciliation, discards old pages and resyncs automatically", async () => {
		jest.useFakeTimers()
		read = async () => ({ data: [child("old")], nextCursor: null })
		await render()
		const pending = deferred<SessionRosterPage>()
		read = async () => pending.promise
		await act(async () => {
			void state.refresh()
			emit({ type: "disconnected" })
		})
		expect(state.sessions).toEqual([child("old")])
		expect(state.error).toContain("Connection lost")
		let calls = 0
		read = async () => {
			calls++
			return { data: [child("reconnected")], nextCursor: null }
		}
		await act(async () => {
			jest.advanceTimersByTime(60_000)
		})
		expect(calls).toBe(0)
		await act(async () => {
			emit({ type: "resync" })
		})
		await act(async () => {
			pending.resolve({ data: [child("obsolete")], nextCursor: null })
		})
		expect(state.sessions).toEqual([child("reconnected")])
		expect(state.error).toBeNull()
	})

	test("connection state reattaches after initially offline and a server switch ignores old callbacks", async () => {
		getDefaultStore().set(serverConnectedAtom, false)
		await render()
		expect(listeners.size).toBe(0)
		expect(state.error).toContain("Connect")
		read = async () => ({ data: [child("old")], nextCursor: null })
		await act(async () => {
			getDefaultStore().set(serverConnectedAtom, true)
		})
		const obsolete = [...listeners][0]
		read = async () => ({ data: [child("new")], nextCursor: null })
		await act(async () => {
			getDefaultStore().set(serverUrlAtom, "https://other")
		})
		await act(async () => {
			obsolete?.({ type: "upsert", session: child("stale") })
		})
		expect(state.sessions).toEqual([child("new")])
		expect(listeners.size).toBe(1)
	})

	test("focus and reconnect during an in-flight scan coalesce one follow-up refresh", async () => {
		const pending = deferred<SessionRosterPage>()
		let calls = 0
		read = async () => {
			calls++
			return calls === 1 ? pending.promise : { data: [child("fresh")], nextCursor: null }
		}
		await render()
		await act(async () => {
			window.dispatchEvent(new Event("focus"))
			emit({ type: "resync" })
			window.dispatchEvent(new Event("focus"))
			pending.resolve({ data: [child("old")], nextCursor: null })
		})
		expect(calls).toBe(2)
		expect(state.sessions).toEqual([child("fresh")])
	})

	test("automatic partial scans retain children already loaded beyond the page cap", async () => {
		let phase = 0
		read = async (params) => {
			const cursor = Number((params as { cursor?: string }).cursor ?? 0)
			return {
				data: [child(phase === 0 ? "deep" : `page${cursor}`)],
				nextCursor: phase === 0 ? null : String(cursor + 1),
			}
		}
		await render()
		phase = 1
		await act(async () => {
			window.dispatchEvent(new Event("focus"))
		})
		expect(state.sessions.map((s) => s.id)).toEqual([
			"deep",
			"page0",
			"page1",
			"page2",
			"page3",
			"page4",
		])
		expect(state.nextCursor).toBe("5")
	})
})
