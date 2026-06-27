import { describe, expect, test } from "bun:test"
import { createStore } from "jotai"
import type { Session } from "../lib/types"
import {
	markSessionReadAtom,
	sessionFamily,
	setSessionStatusAtom,
	upsertSessionAtom,
} from "./sessions"
import { viewedSessionIdAtom } from "./ui"

function session(id = "session-1"): Session {
	return {
		id,
		directory: "/repo",
		title: "Turn status",
		time: { created: 1, updated: 1 },
	}
}

describe("session unread completion state", () => {
	test("marks a background session unread when a running turn completes", () => {
		const store = createStore()
		const info = session()
		store.set(upsertSessionAtom, { session: info, directory: "/repo" })

		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "busy" } })
		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "idle" } })

		expect(store.get(sessionFamily(info.id))).toEqual({
			session: info,
			directory: "/repo",
			status: { type: "idle" },
			permissions: [],
			questions: [],
			branch: undefined,
			worktreePath: undefined,
			worktreeBranch: undefined,
			error: undefined,
			setupPhase: undefined,
			hasUnreadCompletion: true,
		})
	})

	test("does not mark a viewed session unread when its running turn completes", () => {
		const store = createStore()
		const info = session()
		store.set(upsertSessionAtom, { session: info, directory: "/repo" })
		store.set(viewedSessionIdAtom, info.id)

		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "busy" } })
		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "idle" } })

		expect(store.get(sessionFamily(info.id))).toEqual({
			session: info,
			directory: "/repo",
			status: { type: "idle" },
			permissions: [],
			questions: [],
			branch: undefined,
			worktreePath: undefined,
			worktreeBranch: undefined,
			error: undefined,
			setupPhase: undefined,
			hasUnreadCompletion: false,
		})
	})

	test("clears unread state when the session is read or starts another turn", () => {
		const store = createStore()
		const info = session()
		store.set(upsertSessionAtom, { session: info, directory: "/repo" })
		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "busy" } })
		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "idle" } })

		store.set(markSessionReadAtom, info.id)
		expect(store.get(sessionFamily(info.id))?.hasUnreadCompletion).toBe(false)

		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "busy" } })
		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "idle" } })
		expect(store.get(sessionFamily(info.id))?.hasUnreadCompletion).toBe(true)

		store.set(setSessionStatusAtom, { sessionId: info.id, status: { type: "retry" } })
		expect(store.get(sessionFamily(info.id))?.hasUnreadCompletion).toBe(false)
	})
})
