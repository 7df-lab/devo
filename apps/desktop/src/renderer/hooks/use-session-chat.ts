import { useAtomValue } from "jotai"
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import {
	type ChatMessageEntry,
	type ChatTurn,
	groupIntoTurns,
	mergeSessionItems,
} from "../atoms/derived/session-chat"
import { itemsFamily, setItemsAtom } from "../atoms/messages"
import { isMockModeAtom } from "../atoms/mock-mode"
import { appStore } from "../atoms/store"
import { streamingVersionFamily } from "../atoms/streaming"
import { queryClient } from "../lib/query-client"
import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import { getBaseClient, getProjectClient } from "../services/connection-manager"
import { queryKeys } from "./use-devo-data"
import { shouldShowChatLoadingSkeleton } from "./chat-loading"

export type { ChatMessageEntry, ChatTurn }

const EMPTY_ENTRIES: ChatMessageEntry[] = []

const MESSAGES_PER_TURN_ESTIMATE = 20
const INITIAL_TURN_COUNT = 8
const PAGE_TURN_COUNT = 5

const INITIAL_LIMIT = INITIAL_TURN_COUNT * MESSAGES_PER_TURN_ESTIMATE
const PAGE_SIZE = PAGE_TURN_COUNT * MESSAGES_PER_TURN_ESTIMATE

/**
 * Hook to load chat data for a session.
 * Reads Native ItemEnvelope atoms (populated by item.updated events).
 */
export function useSessionChat(
	directory: string | null,
	sessionId: string | null,
	isActive = true,
) {
	const isMockMode = useAtomValue(isMockModeAtom)
	const [loading, setLoading] = useState(false)
	const [loadingEarlier, setLoadingEarlier] = useState(false)
	const [hasEarlierMessages, setHasEarlierMessages] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const syncedRef = useRef<string | null>(null)
	const turnsRef = useRef<ChatTurn[]>([])
	const loadedLimitsRef = useRef(new Map<string, number>())

	const storeItems = useAtomValue(itemsFamily(sessionId ?? ""))
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	const entries: ChatMessageEntry[] = useMemo(() => {
		if (!storeItems || storeItems.length === 0) return EMPTY_ENTRIES
		void streamingVersion
		return mergeSessionItems(storeItems)
	}, [storeItems, streamingVersion])

	const turns = useMemo(() => {
		const result = groupIntoTurns(entries, turnsRef.current)
		turnsRef.current = result
		return result
	}, [entries])

	const fetchAndHydrate = useCallback(
		async (sid: string) => {
			const hasCachedData = (appStore.get(itemsFamily(sid)) ?? []).length > 0
			if (!hasCachedData) {
				setLoading(true)
			}
			setError(null)
			try {
				const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
				if (!client) {
					setError("Not connected to Devo server")
					return
				}

				const limit = loadedLimitsRef.current.get(sid) ?? INITIAL_LIMIT
				const result = await client.session.messages({
					sessionId: sid,
					limit,
				})
				const raw = (result.data ?? []) as Array<{ info: NativeItemEnvelope }>
				loadedLimitsRef.current.set(sid, limit)
				setHasEarlierMessages(raw.length >= limit)

				appStore.set(setItemsAtom, {
					sessionId: sid,
					items: raw.map((m) => m.info),
				})
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

	const loadEarlier = useCallback(async () => {
		if (!isActive || !sessionId || !directory || loadingEarlier || !hasEarlierMessages) return
		const client = getProjectClient(directory)
		if (!client) return

		const currentLimit = loadedLimitsRef.current.get(sessionId) ?? INITIAL_LIMIT
		const nextLimit = currentLimit + PAGE_SIZE

		setLoadingEarlier(true)
		try {
			const result = await client.session.messages({
				sessionId: sessionId,
				limit: nextLimit,
			})
			const raw = (result.data ?? []) as Array<{ info: NativeItemEnvelope }>
			loadedLimitsRef.current.set(sessionId, nextLimit)
			setHasEarlierMessages(raw.length >= nextLimit)
			appStore.set(setItemsAtom, {
				sessionId,
				items: raw.map((m) => m.info),
			})
		} catch (err) {
			console.error("Failed to load earlier messages:", err)
		} finally {
			setLoadingEarlier(false)
		}
	}, [isActive, sessionId, directory, loadingEarlier, hasEarlierMessages])

	useLayoutEffect(() => {
		if (!isActive || !sessionId || isMockMode) return
		if (syncedRef.current === sessionId) return
		syncedRef.current = sessionId
		void fetchAndHydrate(sessionId)
	}, [isActive, sessionId, isMockMode, fetchAndHydrate])

	useEffect(() => {
		if (!isActive) syncedRef.current = null
	}, [isActive])

	return {
		turns,
		loading,
		showLoading: shouldShowChatLoadingSkeleton(loading, turns.length),
		loadingEarlier,
		hasEarlierMessages,
		loadEarlier,
		error,
		refresh: sessionId ? () => fetchAndHydrate(sessionId) : async () => {},
	}
}
