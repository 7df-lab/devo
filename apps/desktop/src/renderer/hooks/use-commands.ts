import { SESSION_HISTORY_UNAVAILABLE } from "@devo-ai/sdk/v2/client"
import { useAtomValue } from "jotai"
import { useCallback, useMemo } from "react"
import { sessionFamily } from "../atoms/sessions"
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

export interface UseSessionRevertResult {
	isReverted: boolean
	revertInfo: Session["revert"] | undefined
	canUndo: boolean
	canRedo: boolean
	undo: () => Promise<string | undefined>
	redo: () => Promise<void>
}

/** Keep legacy callers disabled until a preview/confirmation rollback UI exists. */
export function useSessionRevert(
	_directory: string | null,
	_sessionId: string | null,
): UseSessionRevertResult {
	const unavailable = useCallback(async (): Promise<never> => {
		throw new Error(SESSION_HISTORY_UNAVAILABLE)
	}, [])
	return {
		isReverted: false,
		revertInfo: undefined,
		canUndo: false,
		canRedo: false,
		undo: unavailable,
		redo: unavailable,
	}
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

	const clientCommands = useMemo<AppCommand[]>(() => {
		const cmds: AppCommand[] = []

		cmds.push({
			name: "undo",
			label: "Undo",
			description: SESSION_HISTORY_UNAVAILABLE,
			enabled: canUndo,
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
			description: SESSION_HISTORY_UNAVAILABLE,
			enabled: canRedo,
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
