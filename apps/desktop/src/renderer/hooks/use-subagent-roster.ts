import type { NativeSession, SessionRosterPage } from "@devo-ai/sdk/v2/client"
import { useCallback, useEffect, useRef, useState } from "react"
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

/** Bound each user-requested scan; further pages require an explicit Load more. */
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
			// User forks and independent roots are not subagents. Ephemeral side
			// questions do not have a durable transcript to navigate to.
			if (session.parent?.sessionId === parentId && !session.ephemeral) {
				sessions.set(session.id, session)
			}
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

/** On-demand only: no closed-panel polling, child hydration, or model work. */
export function useSubagentRoster(sessionId: string, directory: string, open: boolean) {
	const [roster, setRoster] = useState<SubagentRoster>(emptyRoster)
	const [loading, setLoading] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const request = useRef<AbortController | null>(null)
	const run = useCallback(
		async (previous = emptyRoster) => {
			if (request.current) return
			const controller = new AbortController()
			request.current = controller
			setLoading(true)
			setError(null)
			try {
				const client = getProjectClient(directory)
				if (!client) throw new Error("Connect to the runtime to view subagents.")
				const next = await readSubagentRoster(
					(cursor) =>
						client.session.roster({
							cwds: [],
							includeChildren: true,
							limit: 200,
							...(cursor ? { cursor } : {}),
						}),
					sessionId,
					controller.signal,
					previous,
				)
				if (!controller.signal.aborted) setRoster(next)
			} catch (cause) {
				if (!controller.signal.aborted)
					setError(cause instanceof Error ? cause.message : String(cause))
			} finally {
				if (request.current === controller) {
					request.current = null
					setLoading(false)
				}
			}
		},
		[directory, sessionId],
	)

	useEffect(() => {
		setRoster(emptyRoster)
		setError(null)
		setLoading(false)
		if (open) void run()
		return () => {
			request.current?.abort()
			request.current = null
		}
	}, [open, run])

	return {
		...roster,
		loading,
		error,
		refresh: () => run(),
		loadMore: () => run(roster),
	}
}
