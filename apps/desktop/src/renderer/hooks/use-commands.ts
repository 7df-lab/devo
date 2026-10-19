import {
	isUserMessageItem,
	userMessageText,
} from "@devo-ai/sdk/v2/client"
import { useAtomValue } from "jotai"
import { useCallback, useMemo } from "react"
import { itemsFamily } from "../atoms/messages"
import { sessionFamily } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { formatShortcut } from "../lib/shortcut-display"
import type { Session } from "../lib/types"
import { getProjectClient } from "../services/connection-manager"
import { useServerCommands } from "./use-devo-data"

// ============================================================
// Types
// ============================================================

export interface AppCommand {
	name: string
	label: string
	description: string
	enabled: boolean
	shortcut?: string
	execute: () => Promise<void>
	source: "client" | "server"
}

// ============================================================
// useSessionRevert — undo/redo logic
// ============================================================

function findUndoTarget(sessionId: string, revertMessageId?: string): string | null {
	const items = appStore.get(itemsFamily(sessionId))
	if (!items || items.length === 0) return null

	let lastUserMsgId: string | null = null
	for (let i = items.length - 1; i >= 0; i--) {
		const item = items[i]
		if (!isUserMessageItem(item)) continue
		if (revertMessageId && item.id >= revertMessageId) continue
		lastUserMsgId = item.id
		break
	}
	return lastUserMsgId
}

function findRedoTarget(sessionId: string, revertMessageId: string): string | null {
	const items = appStore.get(itemsFamily(sessionId))
	if (!items) return null

	let foundRevertPoint = false
	for (const item of items) {
		if (item.id === revertMessageId) {
			foundRevertPoint = true
			continue
		}
		if (foundRevertPoint && isUserMessageItem(item)) {
			return item.id
		}
	}
	return null
}

function getUserMessageText(sessionId: string, messageId: string): string {
	const items = appStore.get(itemsFamily(sessionId))
	const envelope = items?.find((item) => item.id === messageId)
	return envelope ? userMessageText(envelope) : ""
}

export interface UseSessionRevertResult {
	isReverted: boolean
	revertInfo: Session["revert"] | undefined
	canUndo: boolean
	canRedo: boolean
	undo: () => Promise<string | undefined>
	redo: () => Promise<void>
}

export function useSessionRevert(
	directory: string | null,
	sessionId: string | null,
): UseSessionRevertResult {
	const entry = useAtomValue(sessionFamily(sessionId ?? ""))
	const session = entry?.session
	const items = useAtomValue(itemsFamily(sessionId ?? ""))

	const isReverted = !!session?.revert
	const revertInfo = session?.revert

	const canUndo = useMemo(() => {
		if (!directory || !sessionId || !items || items.length === 0) return false
		const target = findUndoTarget(sessionId, revertInfo?.itemId)
		return target !== null
	}, [directory, sessionId, items, revertInfo])

	const canRedo = isReverted

	const undo = useCallback(async (): Promise<string | undefined> => {
		if (!directory || !sessionId) return undefined
		const client = getProjectClient(directory)
		if (!client) return undefined

		const sessionEntry = appStore.get(sessionFamily(sessionId))
		if (sessionEntry?.status?.type === "busy") {
			await client.session.abort({ sessionId: sessionId })
		}

		const targetId = findUndoTarget(sessionId, revertInfo?.itemId)
		if (!targetId) return undefined

		const userText = getUserMessageText(sessionId, targetId)
		await client.session.revert({ sessionId: sessionId })
		return userText
	}, [directory, sessionId, revertInfo])

	const redo = useCallback(async () => {
		if (!directory || !sessionId || !revertInfo) return
		const client = getProjectClient(directory)
		if (!client) return

		const nextTarget = findRedoTarget(sessionId, revertInfo.itemId)
		if (nextTarget) {
			await client.session.revert({ sessionId: sessionId })
		} else {
			await client.session.unrevert({ sessionId: sessionId })
		}
	}, [directory, sessionId, revertInfo])

	return { isReverted, revertInfo, canUndo, canRedo, undo, redo }
}

// ============================================================
// useCommands — unified command registry
// ============================================================

export function useCommands(
	directory: string | null,
	sessionId: string | null,
	options?: {
		onUndoTextRestore?: (text: string) => void
	},
): AppCommand[] {
	const { canUndo, canRedo, undo, redo } = useSessionRevert(directory, sessionId)
	const serverCommands = useServerCommands(directory)
	const entry = useAtomValue(sessionFamily(sessionId ?? ""))
	const sessionStatus = entry?.status
	const isIdle = sessionStatus?.type === "idle" || !sessionStatus
	const undoShortcut = formatShortcut(["mod", "Z"])
	const redoShortcut = formatShortcut(["shift", "mod", "Z"])

	const clientCommands = useMemo<AppCommand[]>(() => {
		const cmds: AppCommand[] = []

		cmds.push({
			name: "undo",
			label: "Undo",
			description: "Undo the last turn and restore file changes",
			enabled: canUndo,
			shortcut: undoShortcut,
			source: "client",
			execute: async () => {
				const text = await undo()
				if (text && options?.onUndoTextRestore) {
					options.onUndoTextRestore(text)
				}
			},
		})

		cmds.push({
			name: "redo",
			label: "Redo",
			description: "Restore previously undone messages",
			enabled: canRedo,
			shortcut: redoShortcut,
			source: "client",
			execute: async () => {
				await redo()
			},
		})

		cmds.push({
			name: "compact",
			label: "Compact",
			description: "Summarize the conversation to save context",
			enabled: !!directory && !!sessionId && isIdle,
			source: "client",
			execute: async () => {
				if (!directory || !sessionId) return
				const client = getProjectClient(directory)
				if (!client) return
				await client.session.summarize({ sessionId: sessionId })
			},
		})

		return cmds
	}, [
		canUndo,
		canRedo,
		undo,
		redo,
		undoShortcut,
		redoShortcut,
		directory,
		sessionId,
		isIdle,
		options?.onUndoTextRestore,
		options,
	])

	const allCommands = useMemo<AppCommand[]>(() => {
		const serverCmds: AppCommand[] = serverCommands.map((cmd) => ({
			name: cmd.name,
			label: cmd.name.charAt(0).toUpperCase() + cmd.name.slice(1),
			description: cmd.description ?? `Run /${cmd.name}`,
			enabled: !!directory && !!sessionId && isIdle,
			source: "server" as const,
			execute: async () => {
				if (!directory || !sessionId) return
				const client = getProjectClient(directory)
				if (!client) return
				await client.session.command({
					sessionId: sessionId,
					command: cmd.name,
					arguments: "",
				})
			},
		}))
		return [...clientCommands, ...serverCmds]
	}, [clientCommands, serverCommands, directory, sessionId, isIdle])

	return allCommands
}
