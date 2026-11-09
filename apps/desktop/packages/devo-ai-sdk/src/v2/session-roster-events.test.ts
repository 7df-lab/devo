import { describe, expect, test } from "bun:test"
import { type SessionRosterChange, SessionRosterEvents } from "./session-roster-events"

describe("validated Native roster notifications", () => {
	test("projects lifecycle, queue and connection events without inferring completion", () => {
		const events = new SessionRosterEvents()
		const changes: SessionRosterChange[] = []
		events.subscribe((change) => changes.push(change))
		for (const [method, params] of [
			[
				"session/statusChanged",
				{
					sessionId: "child",
					status: "active",
					flags: ["waitingUserInput"],
					activity: "working",
					activeTurnId: "turn",
				},
			],
			["session/archived", { sessionId: "child", archived: true }],
			["session/cwdChanged", { sessionId: "child", cwd: "/other" }],
			["queue/updated", { sessionId: "child", change: "removed", queueItemId: "q", queue: [] }],
			["session/deleted", { sessionId: "parent", deletedSessionIds: ["parent", "child"] }],
			[
				"initialized",
				{ connectionId: "connection", protocolVersion: "1", serverInstanceId: "instance" },
			],
		] as const)
			events.handle({ type: "notification", method, params })
		events.handle({ type: "closed" })
		expect(changes).toEqual([
			{
				type: "patch",
				sessionId: "child",
				patch: {
					status: "active",
					flags: ["waitingUserInput"],
					activity: "working",
					activeTurnId: "turn",
				},
			},
			{ type: "patch", sessionId: "child", patch: { archived: true } },
			{ type: "patch", sessionId: "child", patch: { cwd: "/other" } },
			{ type: "patch", sessionId: "child", patch: { queuedCount: 0 } },
			{ type: "delete", sessionIds: ["parent", "child"] },
			{ type: "resync" },
			{ type: "disconnected" },
		])
	})

	test("rejects invalid payloads and unrelated notifications; a throwing listener cannot break delivery", () => {
		const events = new SessionRosterEvents()
		const changes: SessionRosterChange[] = []
		events.subscribe(() => {
			throw new Error("view failed")
		})
		events.subscribe((change) => changes.push(change))
		events.handle({
			type: "notification",
			method: "session/statusChanged",
			params: { sessionId: "child", status: "completed", flags: [] },
		})
		events.handle({
			type: "notification",
			method: "queue/updated",
			params: { sessionId: "child", queue: "invalid" },
		})
		events.handle({ type: "notification", method: "item/agentMessage/delta", params: {} })
		events.handle({ type: "request", method: "session/deleted", params: { sessionId: "child" } })
		expect(changes).toEqual([])
		events.handle({
			type: "notification",
			method: "session/deleted",
			params: { sessionId: "child" },
		})
		expect(changes).toEqual([{ type: "delete", sessionIds: ["child"] }])
		events.clear()
		events.handle({ type: "closed" })
		expect(changes).toHaveLength(1)
	})
})
