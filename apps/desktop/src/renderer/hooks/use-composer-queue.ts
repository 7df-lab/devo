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
): ComposerQueueItem {
	return {
		id: entry.queueItemId,
		origin: entry.origin,
		text: queueEntryText(entry),
		status: action ?? "queued",
		queuedInputId: entry.queueItemId,
		fileCount: countQueueFileParts(entry) || undefined,
		createdAtMs: entry.enqueuedAt ? Date.parse(entry.enqueuedAt) : undefined,
		error,
		canEdit: !entry.recoveryRequired && !entry.input?.some((part) => part.type !== "text"),
		recoveryRequired: entry.recoveryRequired,
	}
}

export function useComposerQueue(sessionId: string, directory: string | null) {
	const entries = useAtomValue(sessionQueueFamily(sessionId))
	const activeTurnId = useAtomValue(sessionActiveTurnFamily(sessionId))
	const [pendingActions, setPendingActions] = useState<Map<string, QueueAction>>(new Map())
	const [actionErrors, setActionErrors] = useState<Map<string, string>>(new Map())
	const [draggingId, setDraggingId] = useState<string | null>(null)
	const pendingIds = useRef(new Set<string>())
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
			if (item.recoveryRequired || pendingIds.current.has(queueScope + item.id)) return
			if (item.origin || !directory || !clientSupportsQueue(directory)) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.steer) {
				setItemError(item.id, "Atomic queue steering is unavailable. Update the server to steer safely.")
				return
			}
			setPending(item.id, "steering")
			setItemError(item.id, null)
			try {
				const result = await client.session.queue.steer({ sessionId, queueItemId: item.id, expectedTurnId: activeTurnId ?? "" })
				if (result.data.outcome === "keptQueued") {
					setItemError(item.id, "The active turn cannot accept this message. Your message is still queued.")
				}
			} catch (err) {
				const message = err instanceof Error ? err.message : "Steer failed"
				setItemError(item.id, message)
				log.error("session.steer failed", { sessionId, queueItemId: item.id }, err)
			} finally {
				setPending(item.id, null)
			}
		},
		[directory, sessionId, activeTurnId, queueScope, setItemError, setPending],
	)

	const retryQueueItem = useCallback(async (item: ComposerQueueItem) => {
		if (!item.recoveryRequired || pendingIds.current.has(queueScope + item.id) || !directory) return
		const client = getProjectClient(directory)
		if (!client?.session?.queue?.retry) {
			setItemError(item.id, "Queue recovery is unavailable. Update the server to retry safely.")
			return
		}
		setPending(item.id, "submitting")
		setItemError(item.id, null)
		try {
			await client.session.queue.retry({ sessionId, queueItemId: item.id })
		} catch (err) {
			setItemError(item.id, err instanceof Error ? err.message : "Retry failed")
		} finally {
			setPending(item.id, null)
		}
	}, [directory, sessionId, queueScope, setItemError, setPending])

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
			if (pendingIds.current.has(queueScope + item.id) || item.recoveryRequired) return null
			if (item.origin || item.canEdit === false || (item.fileCount ?? 0) > 0) return null
			const removed = await removeQueueItem(item)
			return removed ? item.text : null
		},
		[queueScope, removeQueueItem],
	)

	const reorderQueueItem = useCallback(
		async (queueItemId: string, targetQueueItemId: string) => {
			if (!directory || queueItemId === targetQueueItemId || entries.some((entry) => entry.recoveryRequired)) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.update) return
			const item = entries.find((entry) => entry.queueItemId === queueItemId)
			const toIndex = entries.findIndex((entry) => entry.queueItemId === targetQueueItemId)
			if (!item || item.recoveryRequired || toIndex < 0 || entries[toIndex].recoveryRequired || pendingIds.current.has(queueScope + item.queueItemId)) return
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
		retryQueueItem,
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
