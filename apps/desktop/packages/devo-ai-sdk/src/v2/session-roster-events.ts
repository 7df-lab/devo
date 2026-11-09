import type { DevoNativeTransportEvent } from "./client"
import type { Session } from "./generated/native"
import { assertValidProtocolPayload } from "./protocol-validation"

type SessionPatch = Partial<
	Pick<
		Session,
		"status" | "flags" | "activity" | "activeTurnId" | "archived" | "cwd" | "queuedCount"
	>
>
export type SessionRosterChange =
	| { type: "upsert"; session: Session }
	| { type: "patch"; sessionId: string; patch: SessionPatch }
	| { type: "delete"; sessionIds: string[] }
	| { type: "disconnected" }
	| { type: "resync" }

const methods = new Set([
	"initialized",
	"session/created",
	"session/metadataUpdated",
	"session/statusChanged",
	"session/cwdChanged",
	"session/archived",
	"session/deleted",
	"queue/updated",
])

/** Passive fan-out. Never consumes the main event queue or creates control subscriptions. */
export class SessionRosterEvents {
	private listeners = new Set<(change: SessionRosterChange) => void>()

	subscribe = (listener: (change: SessionRosterChange) => void): (() => void) => {
		this.listeners.add(listener)
		return () => {
			this.listeners.delete(listener)
		}
	}

	clear(): void {
		this.listeners.clear()
	}

	handle(event: DevoNativeTransportEvent): void {
		if (this.listeners.size === 0) return
		let change: SessionRosterChange
		if (event.type === "closed") {
			change = { type: "disconnected" }
		} else {
			if (event.type !== "notification" || !event.method || !methods.has(event.method)) return
			let value: Record<string, unknown>
			try {
				value = assertValidProtocolPayload({
					method: event.method,
					direction: "incomingNotification",
					payload: event.params,
				})
			} catch {
				// Invalid wire data must not corrupt this independent Native projection.
				return
			}
			const sessionId = value.sessionId as string
			switch (event.method) {
				case "initialized":
					change = { type: "resync" }
					break
				case "session/created":
				case "session/metadataUpdated":
					change = { type: "upsert", session: value.session as Session }
					break
				case "session/deleted":
					change = {
						type: "delete",
						sessionIds: [...new Set([sessionId, ...((value.deletedSessionIds as string[]) ?? [])])],
					}
					break
				default: {
					let patch: SessionPatch
					switch (event.method) {
						case "session/statusChanged":
							patch = {
								status: value.status as Session["status"],
								flags: value.flags as Session["flags"],
								activeTurnId: value.activeTurnId as Session["activeTurnId"],
								...(value.activity === undefined
									? {}
									: { activity: value.activity as Session["activity"] }),
							}
							break
						case "session/cwdChanged":
							patch = { cwd: value.cwd as string }
							break
						case "session/archived":
							patch = { archived: value.archived as boolean }
							break
						default:
							patch = { queuedCount: (value.queue as unknown[]).length }
					}
					change = { type: "patch", sessionId, patch }
				}
			}
		}
		for (const listener of this.listeners) {
			try {
				listener(change)
			} catch {
				/* A view cannot break transport delivery. */
			}
		}
	}
}
