import { useAtomValue } from "jotai"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { sessionActiveTurnFamily, sessionQueueFamily } from "../atoms/queue"
import { createLogger } from "../lib/logger"
import {
	countQueueFileParts,
	queueEntryText,
	type QueueWireEntry,
} from "../lib/queue-helpers"
import { getProjectClient } from "../services/connection-manager"
import type {
	ComposerQueueItem,
	ComposerQueueItemStatus,
} from "../components/chat/composer-status-stack"

const log = createLogger("use-composer-queue")

type QueueAction = "steering" | "removing" | "submitting"

function wireEntryToComposerItem(
	entry: QueueWireEntry,
	action: QueueAction | null,
	error?: string,
	steered?: boolean,
): ComposerQueueItem {
	return {
		id: entry.queueItemId,
		text: queueEntryText(entry),
		status: action ?? "queued",
		queuedInputId: entry.queueItemId,
		fileCount: countQueueFileParts(entry) || undefined,
		createdAtMs: entry.enqueuedAt ? Date.parse(entry.enqueuedAt) : undefined,
		error,
		canEdit: !steered && !entry.input?.some((part) => part.type !== "text"),
		steered,
	}
}

export function useComposerQueue(sessionId: string, directory: string | null) {
	const entries = useAtomValue(sessionQueueFamily(sessionId))
	const activeTurnId = useAtomValue(sessionActiveTurnFamily(sessionId))
	const [pendingActions, setPendingActions] = useState<Map<string, QueueAction>>(new Map())
	const [actionErrors, setActionErrors] = useState<Map<string, string>>(new Map())
	const [draggingId, setDraggingId] = useState<string | null>(null)
	const pendingIds = useRef(new Set<string>())
	const steeredIds = useRef(new Set<string>())
	const queueScope = JSON.stringify([directory, sessionId])

	const refreshQueue = useCallback(async () => {
		if (!directory) return
		const client = getProjectClient(directory)
		if (!client?.session?.queue?.list) return
		try {
			await client.session.queue.list({ sessionId: sessionId })
		} catch (err) {
			log.error("queue.list failed", { sessionId }, err)
		}
	}, [directory, sessionId])

	useEffect(() => {
		void refreshQueue()
	}, [refreshQueue])

	useEffect(() => {
		setDraggingId(null)
	}, [sessionId])

	const queueItems = useMemo(() => {
		return entries.map((entry) =>
			wireEntryToComposerItem(
				entry,
				pendingActions.get(queueScope + entry.queueItemId) ?? null,
				actionErrors.get(queueScope + entry.queueItemId),
				steeredIds.current.has(queueScope + entry.queueItemId),
			),
		)
	}, [actionErrors, entries, pendingActions, queueScope])

	// The composer is reused across sessions. Keep action state keyed by the
	// original session so navigation cannot reset in-flight locks or retries.
	const setPending = useCallback((queueItemId: string, action: QueueAction | null) => {
		const key = queueScope + queueItemId
		if (action) pendingIds.current.add(key)
		else pendingIds.current.delete(key)
		setPendingActions((current) => {
			const next = new Map(current)
			if (action) next.set(key, action)
			else next.delete(key)
			return next
		})
	}, [queueScope])

	const setItemError = useCallback((queueItemId: string, message: string | null) => {
		const key = queueScope + queueItemId
		setActionErrors((current) => {
			const next = new Map(current)
			if (message) next.set(key, message)
			else next.delete(key)
			return next
		})
	}, [queueScope])

	const steerQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if (pendingIds.current.has(queueScope + item.id)) return
			if (!directory || !clientSupportsQueue(directory)) return
			const client = getProjectClient(directory)
			if (!client?.session?.steer || !client?.session?.queue?.remove) return
			setPending(item.id, "steering")
			setItemError(item.id, null)
			try {
				if (!steeredIds.current.has(queueScope + item.id)) {
					await client.session.steer({ sessionId, queueItemId: item.id })
					// A retry after removal fails must not inject the same message twice.
					steeredIds.current.add(queueScope + item.id)
				}
				await client.session.queue.remove({
					sessionId: sessionId,
					queueItemId: item.id,
				})
			} catch (err) {
				const message = err instanceof Error ? err.message : "Steer failed"
				setItemError(item.id, message)
				log.error("session.steer failed", { sessionId, queueItemId: item.id }, err)
			} finally {
				setPending(item.id, null)
			}
		},
		[directory, sessionId, queueScope, setItemError, setPending],
	)

	const removeQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if (pendingIds.current.has(queueScope + item.id)) return
			if (!directory || !clientSupportsQueue(directory)) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.remove) return
			setPending(item.id, "removing")
			setItemError(item.id, null)
			try {
				await client.session.queue.remove({
					sessionId: sessionId,
					queueItemId: item.id,
				})
				return true
			} catch (err) {
				const message = err instanceof Error ? err.message : "Remove failed"
				setItemError(item.id, message)
				log.error("queue.remove failed", { sessionId, queueItemId: item.id }, err)
				return false
			} finally {
				setPending(item.id, null)
			}
		},
		[directory, sessionId, queueScope, setItemError, setPending],
	)

	const editQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if (pendingIds.current.has(queueScope + item.id) || steeredIds.current.has(queueScope + item.id)) return null
			if (item.canEdit === false || (item.fileCount ?? 0) > 0) return null
			const removed = await removeQueueItem(item)
			return removed ? item.text : null
		},
		[queueScope, removeQueueItem],
	)

	const reorderQueueItem = useCallback(
		async (queueItemId: string, targetQueueItemId: string) => {
			if (!directory || queueItemId === targetQueueItemId) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.update) return
			const item = entries.find((entry) => entry.queueItemId === queueItemId)
			const toIndex = entries.findIndex((entry) => entry.queueItemId === targetQueueItemId)
			if (!item || toIndex < 0 || pendingIds.current.has(queueScope + item.queueItemId)) return
			setPending(item.queueItemId, "submitting")
			setItemError(item.queueItemId, null)
			try {
				await client.session.queue.update({
					sessionId: sessionId,
					queueItemId: item.queueItemId,
					// Native queue positions are one-based; drag indices are zero-based.
					position: toIndex + 1,
				})
			} catch (err) {
				setItemError(item.queueItemId, err instanceof Error ? err.message : "Reorder failed")
				log.error("queue.reorder failed", { sessionId, queueItemId, targetQueueItemId }, err)
				void refreshQueue()
			} finally {
				setPending(item.queueItemId, null)
			}
		},
		[directory, entries, refreshQueue, sessionId, queueScope, setItemError, setPending],
	)

	return {
		queueItems,
		activeTurnId,
		draggingId,
		setDraggingId,
		steerQueueItem,
		removeQueueItem,
		editQueueItem,
		reorderQueueItem,
	}
}

function clientSupportsQueue(directory: string): boolean {
	const client = getProjectClient(directory)
	return !!client?.session?.queue
}

export function mapQueueItemStatus(status: ComposerQueueItemStatus): QueueAction | null {
	switch (status) {
		case "steering":
			return "steering"
		case "removing":
			return "removing"
		case "submitting":
			return "submitting"
		default:
			return null
	}
}
