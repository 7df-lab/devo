import { useAtomValue } from "jotai"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
	type ChatMessageEntry,
	type ChatTurn,
	groupIntoTurns,
	mergeSessionParts,
} from "../atoms/derived/session-chat"
import { messagesFamily, setMessagesAtom } from "../atoms/messages"
import { isMockModeAtom } from "../atoms/mock-mode"
import { partsFamily, partStorageKey } from "../atoms/parts"
import { appStore } from "../atoms/store"
import { streamingVersionFamily } from "../atoms/streaming"
import { queryClient } from "../lib/query-client"
import type { Message, Part } from "../lib/types"
import { getBaseClient, getProjectClient } from "../services/connection-manager"
import { queryKeys } from "./use-devo-data"

// Re-export types for consumers
export type { ChatMessageEntry, ChatTurn }

/** Sentinel empty array — stable reference */
const EMPTY_ENTRIES: ChatMessageEntry[] = []

/** How many messages to fetch on initial load. */
const INITIAL_LIMIT = 30

/**
 * Hook to load chat data for a session.
 *
 * - Reads messages/parts from Jotai atoms (populated by ACP events)
 * - Does a one-time initial fetch to hydrate the store
 * - Uses structural sharing in `groupIntoTurns` to preserve React.memo()
 * - No polling — ACP events keep data up to date
 * - Subscribes to the per-session streaming version so only updates for
 *   THIS session trigger re-renders (not all sessions globally).
 */
export function useSessionChat(
	directory: string | null,
	sessionId: string | null,
	_isActive = false,
) {
	const isMockMode = useAtomValue(isMockModeAtom)
	const [loading, setLoading] = useState(false)
	const [loadingEarlier, setLoadingEarlier] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const syncedRef = useRef<string | null>(null)
	const turnsRef = useRef<ChatTurn[]>([])
	const hasEarlierRef = useRef(false)

	// Read from Jotai atoms
	const storeMessages = useAtomValue(messagesFamily(sessionId ?? ""))
	// Per-session streaming version: only bumped when THIS session streams
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	// Build ChatMessageEntry[] merging streaming overlay
	const entries: ChatMessageEntry[] = useMemo(() => {
		if (!storeMessages || storeMessages.length === 0) return EMPTY_ENTRIES
		return mergeSessionParts(
			sessionId ?? "",
			storeMessages,
			(messageId) => appStore.get(partsFamily(partStorageKey(sessionId ?? "", messageId))),
			streamingVersion,
		)
	}, [storeMessages, streamingVersion, sessionId])

	// Group into turns with structural sharing
	const turns = useMemo(() => {
		const result = groupIntoTurns(entries, turnsRef.current)
		turnsRef.current = result
		return result
	}, [entries])

	// One-time fetch to hydrate the store when session changes
	const fetchAndHydrate = useCallback(
		async (sid: string) => {
			// Only show the loading spinner if the session has no cached data yet.
			// When switching back to a previously-visited session the existing messages
			// remain visible while the background refresh runs, avoiding a jarring flash.
			const hasCachedData = (appStore.get(messagesFamily(sid)) ?? []).length > 0
			if (!hasCachedData) {
				setLoading(true)
			}
			setError(null)
			try {
				// Use a directory-scoped client when available, otherwise fall back to the base client
				const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
				if (!client) {
					setError("Not connected to Devo server")
					return
				}

				const result = await client.session.messages({
					sessionID: sid,
					limit: INITIAL_LIMIT,
				})
				const raw = (result.data ?? []) as Array<{ info: Message; parts: Part[] }>
				hasEarlierRef.current = raw.length >= INITIAL_LIMIT

				// Hydrate the Jotai store
				const messages = raw.map((m) => m.info)
				const parts: Record<string, Part[]> = {}
				for (const m of raw) {
					parts[m.info.id] = m.parts
				}
				appStore.set(setMessagesAtom, { sessionId: sid, messages, parts })
				if (directory) {
					queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) })
					queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) })
				}
			} catch (err) {
				console.error("Failed to fetch session messages:", err)
				setError(err instanceof Error ? err.message : "Failed to load messages")
			} finally {
				setLoading(false)
			}
		},
		[directory],
	)

	// Load all messages (for "load earlier" button)
	const loadEarlier = useCallback(async () => {
		if (!sessionId || !directory || loadingEarlier) return
		const client = getProjectClient(directory)
		if (!client) return

		setLoadingEarlier(true)
		try {
			const result = await client.session.messages({
				sessionID: sessionId,
			})
			const raw = (result.data ?? []) as Array<{ info: Message; parts: Part[] }>
			hasEarlierRef.current = false

			const messages = raw.map((m) => m.info)
			const parts: Record<string, Part[]> = {}
			for (const m of raw) {
				parts[m.info.id] = m.parts
			}
			appStore.set(setMessagesAtom, { sessionId, messages, parts })
			queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) })
			queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) })
		} catch (err) {
			console.error("Failed to load earlier messages:", err)
		} finally {
			setLoadingEarlier(false)
		}
	}, [sessionId, directory, loadingEarlier])

	// Trigger initial fetch when session changes (skip in mock mode -- data is pre-hydrated)
	useEffect(() => {
		if (isMockMode) return
		if (!sessionId) return
		if (syncedRef.current === sessionId) return
		syncedRef.current = sessionId
		fetchAndHydrate(sessionId)
	}, [sessionId, fetchAndHydrate, isMockMode])

	// Reset per-session refs whenever the active session changes.
	//
	// - turnsRef: structural-sharing cache — must be cleared so stale turn objects
	//   from the previous session aren't mixed into the new session's render.
	// - hasEarlierRef: tracks whether the server has older messages to load.
	//   MUST be cleared so the "Load earlier messages" button from a previous
	//   session (that had 30+ messages) doesn't appear on a freshly-switched
	//   session whose atom is still empty.
	// - loading: if the new session has no cached data yet, pre-set the loading
	//   flag so the UI shows a spinner instead of "No messages yet" during the
	//   one render that happens before the fetch effect fires.
	useEffect(() => {
		turnsRef.current = []
		hasEarlierRef.current = false
		if (!isMockMode && sessionId) {
			const hasCachedData = (appStore.get(messagesFamily(sessionId)) ?? []).length > 0
			if (!hasCachedData) {
				setLoading(true)
			}
		}
	}, [sessionId, isMockMode])

	return {
		turns,
		rawMessages: entries,
		loading,
		loadingEarlier,
		error,
		hasEarlierMessages: hasEarlierRef.current,
		loadEarlier,
		reload: fetchAndHydrate,
	}
}
