import { useAtomValue } from "jotai"
import { useCallback } from "react"
import { connectionAtom } from "../atoms/connection"
import { upsertItemAtom } from "../atoms/messages"
import { sessionFamily, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { createLogger } from "../lib/logger"
import type {
	FileAttachment,
	FilePartInput,
	QuestionAnswer,
	PermissionResponse,
	Session,
} from "../lib/types"
import { getProjectClient } from "../services/connection-manager"

const log = createLogger("use-server")

/**
 * Hook for Devo server connection state.
 */
export function useServerConnection() {
	const conn = useAtomValue(connectionAtom)
	return {
		connected: conn.connected,
		url: conn.url,
	}
}

/**
 * Hook for agent actions (stop, approve, deny, etc.).
 */
export function useAgentActions() {
	const abort = useCallback(async (directory: string, sessionId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("abort", { sessionId })
		try {
			await client.session.abort({ sessionId: sessionId })
		} catch (err) {
			log.error("abort failed", { sessionId }, err)
			throw err
		}
	}, [])

	const sendPrompt = useCallback(
		async (
			directory: string,
			sessionId: string,
			text: string,
			options?: {
				model?: { providerID: string; modelID: string }
				agent?: string
				variant?: string
				collaborationMode?: string
				files?: FileAttachment[]
			},
		) => {
			log.debug("sendPrompt called", {
				directory,
				sessionId,
				textLength: text.length,
				agent: options?.agent,
				model: options?.model,
				variant: options?.variant,
				collaborationMode: options?.collaborationMode,
				hasFiles: !!(options?.files && options.files.length > 0),
			})

			const client = getProjectClient(directory)
			if (!client) {
				log.error("sendPrompt: no client for directory", { directory })
				throw new Error("Not connected to Devo server")
			}
			log.debug("sendPrompt: got client", { directory })

			// Optimistic user message is added only when a turn starts immediately.
			// Queued follow-ups stay in the composer queue strip, not the transcript.
			const optimisticId = `optimistic-${Date.now()}`
			const buildOptimistic = () => {
				const content: Array<Record<string, unknown>> = [{ type: "text", text }]
				for (const file of options?.files ?? []) {
					content.push({
						type: "file",
						mime: file.mediaType ?? "application/octet-stream",
						filename: file.filename,
						url: file.url,
					})
				}
				appStore.set(upsertItemAtom, {
					id: optimisticId,
					sessionId,
					turnId: "",
					seq: Number.MAX_SAFE_INTEGER,
					revision: 0,
					createdAt: new Date().toISOString(),
					updatedAt: new Date().toISOString(),
					state: "completed",
					item: {
						type: "userMessage",
						content,
						entry: "turnStart",
					},
				})
			}

			// Build parts array for the API call
			const parts: Array<{ type: "text"; text: string } | FilePartInput> = [{ type: "text", text }]
			for (const file of options?.files ?? []) {
				parts.push({
					type: "file",
					mime: file.mediaType ?? "application/octet-stream",
					filename: file.filename,
					url: file.url,
				})
			}

			log.debug("sendPrompt: calling promptAsync", {
				sessionId,
				agent: options?.agent,
				model: options?.model,
				partsCount: parts.length,
			})
			try {
				const result = await client.session.promptAsync({
					sessionId: sessionId,
					parts,
					model: options?.model
						? { providerID: options.model.providerID, modelID: options.model.modelID }
						: undefined,
					agent: options?.agent,
					variant: options?.variant,
					collaborationMode: options?.collaborationMode,
				})
				if (result.data?.outcome !== "queued") {
					buildOptimistic()
				}
				log.debug("sendPrompt: promptAsync returned", {
					sessionId,
					outcome: result.data?.outcome,
					result: JSON.stringify(result ?? null).slice(0, 200),
				})
			} catch (err) {
				log.error("sendPrompt: promptAsync failed", { sessionId, agent: options?.agent }, err)
				throw err
			}
		},
		[],
	)

	const createSession = useCallback(async (directory: string, title?: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("createSession", { directory, title })
		try {
			const result = await client.session.create({ title })
			const session = result.data
			if (session) {
				appStore.set(upsertSessionAtom, { session, directory })
			}
			log.debug("createSession succeeded", { sessionId: session?.id })
			return session
		} catch (err) {
			log.error("createSession failed", { directory, title }, err)
			throw err
		}
	}, [])

	const renameSession = useCallback(async (directory: string, sessionId: string, title: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("renameSession", { sessionId, title })

		// Optimistic update
		const entry = appStore.get(sessionFamily(sessionId))
		if (entry) {
			appStore.set(upsertSessionAtom, {
				session: { ...entry.session, title },
				directory: entry.directory,
			})
		}

		try {
			await client.session.update({ sessionId: sessionId, title })
		} catch (err) {
			log.error("renameSession failed", { sessionId, title }, err)
			throw err
		}
	}, [])

	const deleteSession = useCallback(async (directory: string, sessionId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("deleteSession", { sessionId })
		try {
			await client.session.delete({ sessionId: sessionId })
		} catch (err) {
			log.error("deleteSession failed", { sessionId }, err)
			throw err
		}
	}, [])

	const respondToPermission = useCallback(
		async (
			directory: string,
			sessionId: string,
			permissionId: string,
			response: PermissionResponse,
		) => {
			const client = getProjectClient(directory)
			if (!client) throw new Error("Not connected to Devo server")
			log.debug("respondToPermission", { sessionId, permissionId, response })
			try {
				await client.permission.respond({
					sessionId: sessionId,
					permissionId: permissionId,
					response,
				})
			} catch (err) {
				log.error("respondToPermission failed", { sessionId, permissionId, response }, err)
				throw err
			}
		},
		[],
	)

	const replyToQuestion = useCallback(
		async (directory: string, requestId: string, answers: QuestionAnswer[]) => {
			const client = getProjectClient(directory)
			if (!client) throw new Error("Not connected to Devo server")
			log.debug("replyToQuestion", { requestId })
			try {
				await client.question.reply({ requestId: requestId, answers })
			} catch (err) {
				log.error("replyToQuestion failed", { requestId }, err)
				throw err
			}
		},
		[],
	)

	const rejectQuestion = useCallback(async (directory: string, requestId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("rejectQuestion", { requestId })
		try {
			await client.question.reject({ requestId: requestId })
		} catch (err) {
			log.error("rejectQuestion failed", { requestId }, err)
			throw err
		}
	}, [])

	const revert = useCallback(async (directory: string, sessionId: string, messageId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("revert", { sessionId, messageId })
		try {
			const entry = appStore.get(sessionFamily(sessionId))
			if (entry?.status?.type === "busy") {
				log.debug("revert: aborting busy session first", { sessionId })
				await client.session.abort({ sessionId: sessionId })
			}
			await client.session.revert({ sessionId: sessionId })
		} catch (err) {
			log.error("revert failed", { sessionId, messageId }, err)
			throw err
		}
	}, [])

	const unrevert = useCallback(async (directory: string, sessionId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("unrevert", { sessionId })
		try {
			await client.session.unrevert({ sessionId: sessionId })
		} catch (err) {
			log.error("unrevert failed", { sessionId }, err)
			throw err
		}
	}, [])

	const executeCommand = useCallback(
		async (directory: string, sessionId: string, command: string, args: string) => {
			const client = getProjectClient(directory)
			if (!client) throw new Error("Not connected to Devo server")
			log.debug("executeCommand", { sessionId, command })
			try {
				await client.session.command({
					sessionId: sessionId,
					command,
					arguments: args,
				})
			} catch (err) {
				log.error("executeCommand failed", { sessionId, command }, err)
				throw err
			}
		},
		[],
	)

	const summarize = useCallback(async (directory: string, sessionId: string) => {
		const client = getProjectClient(directory)
		if (!client) throw new Error("Not connected to Devo server")
		log.debug("summarize", { sessionId })
		try {
			await client.session.summarize({ sessionId: sessionId })
		} catch (err) {
			log.error("summarize failed", { sessionId }, err)
			throw err
		}
	}, [])

	const forkSession = useCallback(
		async (
			directory: string,
			sessionId: string,
			options?: { atTurnId?: string; cut?: "through" | "before" },
		): Promise<Session> => {
			const client = getProjectClient(directory)
			if (!client) throw new Error("Not connected to Devo server")
			log.debug("forkSession", { sessionId, options })
			try {
				const result = await client.session.fork({
					sessionId: sessionId,
					atTurnId: options?.atTurnId,
					cut: options?.cut,
				})
				const session = result.data as Session
				if (session) {
					appStore.set(upsertSessionAtom, { session, directory })
				}
				log.debug("forkSession succeeded", { forkedSessionId: session?.id })
				return session
			} catch (err) {
				log.error("forkSession failed", { sessionId, options }, err)
				throw err
			}
		},
		[],
	)

	const editMessage = useCallback(
		async (directory: string, sessionId: string, messageId: string, text: string) => {
			const client = getProjectClient(directory)
			if (!client) throw new Error("Not connected to Devo server")
			log.debug("editMessage", { sessionId, messageId, textLength: text.length })
			try {
				await client.session.editMessage({
					sessionId: sessionId,
					itemId: messageId,
					text,
				})
				log.debug("editMessage succeeded", { sessionId, messageId })
			} catch (err) {
				log.error("editMessage failed", { sessionId, messageId }, err)
				throw err
			}
		},
		[],
	)

	return {
		abort,
		sendPrompt,
		createSession,
		renameSession,
		deleteSession,
		respondToPermission,
		replyToQuestion,
		rejectQuestion,
		revert,
		unrevert,
		executeCommand,
		summarize,
		forkSession,
		editMessage,
	}
}
