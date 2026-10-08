import { toast } from "sonner"
import { createLogger } from "../../lib/logger"
import { queryClient } from "../../lib/query-client"
import type { Event } from "../../lib/types"
import type { ContextOccupancy } from "../../lib/context-occupancy"
import { compactionStatusFamily } from "../compaction"
import { serverConnectedAtom } from "../connection"
import { discoveryAtom } from "../discovery"
import { removeItemAtom, upsertItemAtom } from "../messages"
import {
	addPermissionAtom,
	addQuestionAtom,
	removePermissionAtom,
	removeQuestionAtom,
	removeSessionAtom,
	setProviderRetryStatusAtom,
	setSessionErrorAtom,
	setSessionStatusAtom,
	upsertSessionAtom,
} from "../sessions"
import { setSessionActiveTurnAtom, setSessionQueueAtom } from "../queue"
import { sessionNativeFamily } from "../session-native"
import { appStore } from "../store"
import { streamingVersionFamily } from "../streaming"
import { todosFamily } from "../todos"
import { setSessionDiffAtom } from "../ui"
import { applyWorkspaceChangesUpdatedAtom } from "../workspace-changes"

const log = createLogger("event-processor")

/**
 * Invalidate all Devo data queries for a specific directory.
 * Called when an instance is disposed so the UI re-fetches config, agents, providers, etc.
 */
function invalidateDirectoryQueries(directory: string): void {
	log.info("Invalidating queries for disposed instance", { directory })
	for (const key of ["config", "providers", "agents", "commands", "vcs"]) {
		queryClient.invalidateQueries({ queryKey: [key, directory] })
	}
}

/**
 * Invalidate all Devo data queries across all directories.
 * Called when a global dispose event occurs (e.g. global config change).
 */
function invalidateAllQueries(): void {
	log.info("Invalidating all Devo queries (global dispose)")
	for (const key of ["config", "providers", "agents", "commands", "vcs"]) {
		queryClient.invalidateQueries({ queryKey: [key] })
	}
}

/**
 * Central Native event dispatcher.
 * A standalone function that writes to Jotai atoms via the store API.
 * Called by the event batcher in connection-manager.
 */
export function processEvent(event: Event): void {
	const { set } = appStore

	switch (event.type) {
		case "server.connected":
			set(serverConnectedAtom, true)
			break

		case "server.instance.disposed": {
			const directory = event.properties.directory
			if (directory) {
				invalidateDirectoryQueries(directory)
			}
			break
		}

		case "global.disposed":
			invalidateAllQueries()
			break

		case "project.updated": {
			const project = event.properties
			if (project.id && project.worktree) {
				const current = appStore.get(discoveryAtom)
				const existing = current.projects.findIndex((p) => p.id === project.id)
				const nextProjects =
					existing >= 0
						? current.projects.map((p, i) => (i === existing ? project : p))
						: [...current.projects, project]
				set(discoveryAtom, { ...current, projects: nextProjects })
			}
			break
		}

		case "session.created": {
			const info = event.properties.info
			set(upsertSessionAtom, { session: info, directory: info.directory ?? "" })
			break
		}

		case "session.updated": {
			const info = event.properties.info
			set(upsertSessionAtom, { session: info, directory: info.directory ?? "" })
			break
		}

		case "session.deleted":
			set(removeSessionAtom, event.properties.info.id)
			break

		case "turn.provider_retry_status": {
			const properties = event.properties
			const sessionId = properties.sessionId
			const turnId = properties.turnId
			if (sessionId && turnId) {
				const phase = String(properties.phase ?? "")
				set(setProviderRetryStatusAtom, {
					sessionId,
					status:
						phase === "resumed"
							? undefined
							: {
								turnId,
								attempt: Number(properties.attempt ?? 0),
								backoffMs: Number(properties.backoffMs ?? 0),
								provider: String(properties.provider ?? ""),
								model: String(properties.model ?? ""),
								phase,
								message: String(properties.message ?? ""),
							},
				})
			}
			break
		}

		case "session.status":
			set(setSessionStatusAtom, {
				sessionId: event.properties.sessionId,
				status: event.properties.status,
			})
			// Clear error when session starts working again
			if (event.properties.status.type !== "idle") {
				set(setSessionErrorAtom, {
					sessionId: event.properties.sessionId,
					error: undefined,
				})
			}
			break

		case "session.activeTurn":
			set(setSessionActiveTurnAtom, {
				sessionId: event.properties.sessionId,
				turnId: event.properties.turnId ?? null,
			})
			break

		case "session.queue.updated":
			set(setSessionQueueAtom, {
				sessionId: event.properties.sessionId,
				entries: event.properties.entries ?? [],
			})
			break

		case "session.error": {
			const { sessionId, turnId, error } = event.properties
			if (sessionId && error) {
				set(setSessionErrorAtom, {
					sessionId,
					turnId,
					error: { name: error.name, data: error.data },
				})
			}
			break
		}

		case "session.compaction.started":
		case "session/compaction/started": {
			const sessionId = event.properties.sessionId
			if (sessionId) {
				set(compactionStatusFamily(sessionId), "started")
			}
			break
		}

		case "session.compaction.completed":
		case "session/compaction/completed": {
			const sessionId = event.properties.sessionId
			if (sessionId) {
				// Transcript markers carry the durable "completed" row; clear the
				// live atom so a later compaction can show "started" again.
				set(compactionStatusFamily(sessionId), null)
			}
			break
		}

		case "session.compaction.failed":
		case "session/compaction/failed": {
			const sessionId = event.properties.sessionId
			if (sessionId) {
				set(compactionStatusFamily(sessionId), null)
			}
			break
		}

		case "permission.asked":
			set(addPermissionAtom, {
				sessionId: event.properties.sessionId,
				permission: event.properties,
			})
			break

		case "permission.replied":
			set(removePermissionAtom, {
				sessionId: event.properties.sessionId,
				permissionId: event.properties.requestId,
			})
			break

		case "question.asked":
			set(addQuestionAtom, {
				sessionId: event.properties.sessionId,
				question: event.properties,
			})
			break

		case "question.replied":
			set(removeQuestionAtom, {
				sessionId: event.properties.sessionId,
				requestId: event.properties.requestId,
			})
			break

		case "question.rejected":
			set(removeQuestionAtom, {
				sessionId: event.properties.sessionId,
				requestId: event.properties.requestId,
			})
			break

		case "item.updated":
			set(upsertItemAtom, event.properties.info)
			set(streamingVersionFamily(event.properties.info.sessionId), (v) => v + 1)
			break

		case "item.removed":
			set(removeItemAtom, {
				sessionId: event.properties.sessionId,
				itemId: event.properties.itemId,
			})
			set(streamingVersionFamily(event.properties.sessionId), (v) => v + 1)
			break

		case "todo.updated":
			set(todosFamily(event.properties.sessionId), event.properties.todos)
			break

		case "session.commands.updated": {
			const sessionId = event.properties.sessionId
			if (!sessionId) break
			const current = appStore.get(sessionNativeFamily(sessionId))
			set(sessionNativeFamily(sessionId), {
				...current,
				commands: event.properties.commands ?? [],
			})
			break
		}

		case "session.config.updated": {
			const sessionId = event.properties.sessionId
			if (!sessionId) break
			const current = appStore.get(sessionNativeFamily(sessionId))
			set(sessionNativeFamily(sessionId), {
				...current,
				configOptions: event.properties.configOptions ?? [],
			})
			break
		}

		case "session.mode.updated": {
			const sessionId = event.properties.sessionId
			if (!sessionId) break
			const current = appStore.get(sessionNativeFamily(sessionId))
			set(sessionNativeFamily(sessionId), {
				...current,
				modeID: event.properties.modeID,
			})
			break
		}

		case "session.usage.updated": {
			const sessionId = event.properties.sessionId
			if (!sessionId) break
			const current = appStore.get(sessionNativeFamily(sessionId))
			const nextUsed = Number(event.properties.used ?? 0)
			const nextSize = Number(event.properties.size ?? 0)
			const previousSize = Number(current.usage?.size ?? 0)
			const occupancyWindow = Number(current.occupancy?.contextWindowTokens ?? 0)
			// Server size is already the model effective window. Keep the
			// denominator in sync with live turn updates (including increases
			// after the user raises usable context).
			const stableSize = nextSize > 0 ? nextSize : occupancyWindow > 0 ? occupancyWindow : previousSize
			const nextOccupancy =
				current.occupancy && stableSize > 0 && current.occupancy.contextWindowTokens !== stableSize
					? { ...current.occupancy, contextWindowTokens: stableSize }
					: current.occupancy
			set(sessionNativeFamily(sessionId), {
				...current,
				occupancy: nextOccupancy,
				usage: {
					used: nextUsed,
					size: stableSize,
					cost: event.properties.cost,
				},
			})
			break
		}

		case "context.usage.updated": {
			const sessionId = event.properties.sessionId
			if (!sessionId) break
			// NativeClient normalizes protocol occupancy to this numeric renderer shape.
			const occupancy = event.properties.occupancy as ContextOccupancy | undefined
			const current = appStore.get(sessionNativeFamily(sessionId))
			const occupancyTotal = Number(occupancy?.totalTokens ?? 0)
			const occupancyWindow = Number(occupancy?.contextWindowTokens ?? 0)
			const previousUsed = Number(current.usage?.used ?? 0)
			const previousOccupancyWindow = Number(current.occupancy?.contextWindowTokens ?? 0)
			// Trust the server window (model effective). Shrinks and increases
			// both apply immediately so the Context usage popover denominator
			// stays current.
			const nextWindow = occupancyWindow > 0 ? occupancyWindow : previousOccupancyWindow
			set(sessionNativeFamily(sessionId), {
				...current,
				occupancy: occupancy,
				usage: {
					used: occupancyTotal > 0 ? occupancyTotal : previousUsed,
					size: nextWindow > 0 ? nextWindow : Number(current.usage?.size ?? 0),
					cost: current.usage?.cost,
				},
			})
			break
		}

		case "session.diff": {
			const { sessionId, diff } = event.properties as {
				sessionId: string
				diff: import("../../lib/types").FileDiff[]
			}
			if (sessionId && diff) {
				set(setSessionDiffAtom, { sessionId, diffs: diff })
			}
			break
		}

		case "workspace.changes.updated":
			set(applyWorkspaceChangesUpdatedAtom, event.properties)
			break

		case "provider.authStale": {
			const providerId = String(
				event.properties?.providerId ?? event.properties?.provider_id ?? "",
			).trim()
			const reason =
				typeof event.properties?.reason === "string" && event.properties.reason.trim().length > 0
					? event.properties.reason.trim()
					: undefined
			log.warn("Provider auth stale", { providerId, reason })
			queryClient.invalidateQueries({ queryKey: ["providers"] })
			const label = providerId.length > 0 ? providerId : "provider"
			toast.warning(`Sign in again for ${label}`, {
				description: reason ?? "Credentials expired or refresh failed. Open Settings → Connections.",
			})
			break
		}

		// --- Worktree lifecycle events (from Devo experimental API) ---

		case "worktree.ready":
			log.info("Worktree ready", {
				name: event.properties.name,
				branch: event.properties.branch,
			})
			break

		case "worktree.failed":
			log.warn("Worktree creation failed", {
				message: event.properties.message,
			})
			break
	}
}
