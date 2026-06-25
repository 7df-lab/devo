import { useCallback, useEffect, useRef, useState } from "react"
import type { Activity } from "../lib/types"
import { getBaseClient, getProjectClient } from "../services/connection-manager"

/**
 * SDK returns messages as { info: Message, parts: Part[] }
 * We flatten these for processing.
 */
interface MessageEntry {
	info: {
		id: string
		role: string
		time?: {
			created?: number
		}
	}
	parts: MessagePart[]
}

/**
 * Part types from Devo messages.
 */
interface MessagePart {
	type: string // "text" | "tool" | "step-start" | "step-finish"
	text?: string
	tool?: string
	state?: {
		status?: string
		title?: string
		error?: string
	}
	time?: {
		start?: number
		end?: number
	}
}

/**
 * Maps an Devo tool name to our Activity type.
 */
function toolToActivityType(
	tool: string,
): "read" | "search" | "edit" | "run" | "think" | "write" | "tool" {
	switch (tool) {
		case "read":
			return "read"
		case "glob":
		case "grep":
		case "brightdata_search_engine":
		case "brightdata_search_engine_batch":
			return "search"
		case "edit":
			return "edit"
		case "write":
			return "write"
		case "bash":
			return "run"
		case "todowrite":
		case "todoread":
			return "think"
		default:
			if (tool.includes("search") || tool.includes("grep") || tool.includes("find")) {
				return "search"
			}
			if (tool.includes("fetch") || tool.includes("scrape") || tool.includes("browse")) {
				return "search"
			}
			return "tool"
	}
}

/**
 * Formats a timestamp (milliseconds) to HH:MM format.
 */
function formatTime(ms: number): string {
	const date = new Date(ms)
	return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
}

/**
 * Convert Devo messages to Activity entries for the detail panel.
 */
function messagesToActivities(entries: MessageEntry[]): Activity[] {
	const activities: Activity[] = []
	let activityIndex = 0

	for (const entry of entries) {
		const { info, parts } = entry
		if (info.role === "user") continue // Skip user messages for activity feed

		for (const part of parts) {
			if (part.type === "tool" && part.tool) {
				const title = part.state?.title || part.tool
				const status = part.state?.status || "unknown"
				const timestamp = part.time?.start
					? formatTime(part.time.start)
					: info.time?.created
						? formatTime(info.time.created)
						: ""

				activities.push({
					id: `${info.id}-${activityIndex++}`,
					timestamp,
					type: toolToActivityType(part.tool),
					description: title,
					detail:
						status === "error" && part.state?.error
							? `Error: ${part.state.error.slice(0, 200)}`
							: undefined,
				})
			} else if (part.type === "text" && part.text && info.role !== "user") {
				// Only show text parts that are reasonably short (summaries)
				const text = part.text.trim()
				if (text.length > 0 && text.length < 500) {
					const timestamp = info.time?.created ? formatTime(info.time.created) : ""
					activities.push({
						id: `${info.id}-${activityIndex++}`,
						timestamp,
						type: "think",
						description: text.length > 120 ? `${text.slice(0, 120)}...` : text,
					})
				}
			}
		}
	}

	return activities
}

/**
 * Hook to load messages for a selected session.
 * Fetches on-demand when directory/sessionId changes.
 */
export function useSessionMessages(directory: string | null, sessionId: string | null) {
	const [activities, setActivities] = useState<Activity[]>([])
	const [loading, setLoading] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const abortRef = useRef<AbortController | null>(null)

	const loadMessages = useCallback(async () => {
		if (!sessionId) {
			setActivities([])
			return
		}

		// Cancel any in-flight request
		abortRef.current?.abort()
		const abort = new AbortController()
		abortRef.current = abort

		setLoading(true)
		setError(null)

		try {
			// Use a directory-scoped client when available, otherwise fall back to the base client
			const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
			if (!client) {
				setError("Not connected to Devo server")
				setActivities([])
				return
			}

			const result = await client.session.messages({
				sessionID: sessionId,
			})
			const messages = (result.data as unknown as MessageEntry[]) ?? []

			if (abort.signal.aborted) return

			const derived = messagesToActivities(messages)
			setActivities(derived)
		} catch (err) {
			if (abort.signal.aborted) return
			console.error("Failed to load messages:", err)
			setError(err instanceof Error ? err.message : "Failed to load messages")
			setActivities([])
		} finally {
			if (!abort.signal.aborted) {
				setLoading(false)
			}
		}
	}, [directory, sessionId])

	useEffect(() => {
		loadMessages()
		return () => {
			abortRef.current?.abort()
		}
	}, [loadMessages])

	return { activities, loading, error, reload: loadMessages }
}
