import type { NativeSession, SessionRosterChange, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { useAtomValue } from "jotai"
import { useEffect, useRef, useState } from "react"
import { activeServerIdAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { getProjectClient } from "../services/connection-manager"

export interface SubagentRoster {
	sessions: NativeSession[]
	nextCursor: string | null
	scanned: number
}

const emptyRoster: SubagentRoster = {
	sessions: [],
	nextCursor: null,
	scanned: 0,
}

/** Bound each scan; further pages require an explicit Load more. */
export async function readSubagentRoster(
	read: (cursor?: string) => Promise<SessionRosterPage>,
	parentId: string,
	signal: AbortSignal,
	previous: SubagentRoster = emptyRoster,
): Promise<SubagentRoster> {
	const sessions = new Map(previous.sessions.map((session) => [session.id, session]))
	let cursor = previous.nextCursor ?? undefined
	let scanned = previous.scanned
	const seen = new Set<string>()
	for (let page = 0; page < 5; page++) {
		signal.throwIfAborted()
		if (cursor) seen.add(cursor)
		const result = await read(cursor)
		signal.throwIfAborted()
		for (const session of result.data) {
			// The Native filter is part of the contract, not a hint. Never fall
			// back to scanning the global roster if a mismatched runtime ignores it.
			if (session.parent?.sessionId !== parentId || session.ephemeral)
				throw new Error(
					"The runtime returned an unscoped subagent list. Update the runtime and retry.",
				)
			sessions.set(session.id, session)
		}
		scanned += result.data.length
		cursor = result.nextCursor ?? undefined
		if (!cursor) break
		if (seen.has(cursor))
			throw new Error("The server repeated a session page. Refresh to try again.")
	}
	return {
		sessions: [...sessions.values()],
		nextCursor: cursor ?? null,
		scanned,
	}
}

/** Existing events are immediate; reconcile gaps only while the panel is open. */
export const SUBAGENT_RECONCILE_MS = 30_000

type RosterMutation = Exclude<SessionRosterChange, { type: "disconnected" | "resync" }>

function applyChange(
	roster: SubagentRoster,
	change: RosterMutation,
	parentId: string,
): SubagentRoster {
	const sessions = new Map(roster.sessions.map((session) => [session.id, session]))
	if (change.type === "delete") {
		for (const id of change.sessionIds) sessions.delete(id)
	} else if (change.type === "upsert") {
		const session = change.session
		if (session.parent?.sessionId === parentId && !session.ephemeral)
			sessions.set(session.id, session)
		else sessions.delete(session.id)
	} else {
		const session = sessions.get(change.sessionId)
		if (session) sessions.set(session.id, { ...session, ...change.patch })
	}
	return { ...roster, sessions: [...sessions.values()] }
}

export type SubagentStopState = { status: "pending" } | { status: "error"; message: string }

export function useSubagentRoster(sessionId: string, directory: string, open: boolean) {
	const connected = useAtomValue(serverConnectedAtom)
	const serverUrl = useAtomValue(serverUrlAtom)
	const serverId = useAtomValue(activeServerIdAtom)
	const [roster, setRoster] = useState<SubagentRoster>(emptyRoster)
	const [loading, setLoading] = useState(false)
	const [error, setError] = useState<string | null>(null)
	// Keep in-flight controls across Close/Reopen, separately from read lifetimes.
	// Include the server and parent so late results never target another roster.
	const controlScope = JSON.stringify([sessionId, directory, serverUrl, serverId])
	const stops = useRef(new Map<string, SubagentStopState>())
	const [stopStates, setStopStates] = useState(stops.current)
	const scope = useRef("")
	const current = useRef(emptyRoster)
	const actions = useRef({
		refresh: async () => {},
		loadMore: async () => {},
		stop: async (_id: string) => {},
	})

	useEffect(() => {
		const key = JSON.stringify([open, sessionId, directory, serverUrl, serverId])
		if (scope.current !== key) {
			scope.current = key
			current.current = emptyRoster
			setRoster(emptyRoster)
			setError(null)
		}
		setLoading(false)
		if (!open) return
		if (!connected) {
			setError("Connect to the runtime to view subagents.")
			return
		}
		const client = getProjectClient(directory)
		if (!client) {
			setError("Connect to the runtime to view subagents.")
			return
		}

		let active = true
		let online = true
		let request: AbortController | null = null
		let timer: ReturnType<typeof setTimeout> | undefined
		let pendingRefresh = false
		// Keep only the latest overlay per ID while a snapshot is in flight.
		// Old pages cannot overwrite newer status, insertions or tombstones.
		const changes = new Map<string, RosterMutation>()
		const publish = (next: SubagentRoster) => {
			current.current = next
			setRoster(next)
		}
		const run = async (more = false) => {
			if (!active || !online || request) return
			const controller = new AbortController()
			request = controller
			changes.clear()
			clearTimeout(timer)
			setLoading(true)
			setError(null)
			try {
				let next = await readSubagentRoster(
					(cursor) =>
						client.session.roster({
							cwds: [],
							parentSessionId: sessionId,
							limit: 200,
							...(cursor ? { cursor } : {}),
						}),
					sessionId,
					controller.signal,
					more ? current.current : emptyRoster,
				)
				if (!active || controller.signal.aborted) return
				// A capped scan is not evidence that previously loaded children vanished.
				if (next.nextCursor) {
					const sessions = new Map(current.current.sessions.map((s) => [s.id, s]))
					for (const session of next.sessions) sessions.set(session.id, session)
					next = { ...next, sessions: [...sessions.values()] }
				}
				for (const change of changes.values()) next = applyChange(next, change, sessionId)
				publish(next)
			} catch (cause) {
				if (active && !controller.signal.aborted)
					setError(cause instanceof Error ? cause.message : String(cause))
			} finally {
				if (active && request === controller) {
					request = null
					changes.clear()
					setLoading(false)
					if (online) {
						if (pendingRefresh) {
							pendingRefresh = false
							void run()
						} else timer = setTimeout(() => void run(), SUBAGENT_RECONCILE_MS)
					}
				}
			}
		}
		const refresh = () => {
			if (request) pendingRefresh = true
			else void run()
		}
		// Register before the first read, rather than stealing from the single-consumer stream.
		const unsubscribe = client.session.observeRoster((change: SessionRosterChange) => {
			if (!active) return
			if (change.type === "disconnected") {
				online = false
				clearTimeout(timer)
				request?.abort()
				request = null
				changes.clear()
				pendingRefresh = false
				setLoading(false)
				setError("Connection lost. Updates will resume when the runtime reconnects.")
				return
			}
			if (change.type === "resync") {
				online = true
				refresh()
				return
			}
			if (!online) return
			if (request) {
				if (change.type === "delete") {
					for (const id of change.sessionIds) changes.set(id, { type: "delete", sessionIds: [id] })
				} else if (change.type === "upsert") changes.set(change.session.id, change)
				else {
					const previous = changes.get(change.sessionId)
					if (previous?.type === "upsert")
						changes.set(change.sessionId, {
							type: "upsert",
							session: { ...previous.session, ...change.patch },
						})
					else if (previous?.type !== "delete")
						changes.set(change.sessionId, {
							...change,
							patch: {
								...(previous?.type === "patch" ? previous.patch : {}),
								...change.patch,
							},
						})
				}
			}
			publish(applyChange(current.current, change, sessionId))
		})
		actions.current = {
			refresh: () => run(),
			loadMore: () => (current.current.nextCursor ? run(true) : Promise.resolve()),
			stop: async (id: string) => {
				const child = current.current.sessions.find((session) => session.id === id)
				const key = JSON.stringify([controlScope, id])
				if (
					!active ||
					!online ||
					!child ||
					child.status !== "active" ||
					child.archived ||
					child.parent?.sessionId !== sessionId ||
					child.ephemeral ||
					stops.current.get(key)?.status === "pending"
				)
					return
				const publishStop = (state: SubagentStopState | null) => {
					stops.current = new Map(stops.current)
					if (state) stops.current.set(key, state)
					else stops.current.delete(key)
					setStopStates(stops.current)
				}
				publishStop({ status: "pending" })
				try {
					// Use this captured client, never resolve a different server after awaiting.
					await client.session.abort({ sessionId: id })
					// An ACK is not proof of task completion. Status stays server-owned.
					publishStop(null)
				} catch (cause) {
					publishStop({
						status: "error",
						message: `Stop was not confirmed. Check the child's state before retrying. ${cause instanceof Error ? cause.message : String(cause)}`,
					})
				}
				if (active && online) refresh()
			},
		}
		window.addEventListener("focus", refresh)
		void run()
		return () => {
			active = false
			clearTimeout(timer)
			request?.abort()
			unsubscribe()
			window.removeEventListener("focus", refresh)
			actions.current = { refresh: async () => {}, loadMore: async () => {}, stop: async () => {} }
		}
	}, [open, sessionId, directory, connected, serverUrl, serverId, controlScope])

	return {
		...roster,
		loading,
		error,
		refresh: () => actions.current.refresh(),
		loadMore: () => actions.current.loadMore(),
		stop: (id: string) => actions.current.stop(id),
		canStop: connected && !error,
		stopState: (id: string) => stopStates.get(JSON.stringify([controlScope, id])),
	}
}
