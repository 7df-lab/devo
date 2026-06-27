import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { Agent } from "../../lib/types"
import {
	SessionDeleteDialogBody,
	deleteSessionNavigationTarget,
} from "./sidebar-session-delete"

function agent(): Agent {
	return {
		id: "session-1",
		sessionId: "session-1",
		name: "Greeting and Introduction",
		status: "idle",
		environment: "local",
		project: "devo",
		projectSlug: "devo-123",
		directory: "/Users/tsiao/Desktop/devo",
		projectDirectory: "/Users/tsiao/Desktop/devo",
		branch: "main",
		duration: "42m",
		activities: [],
		permissions: [],
		questions: [],
		createdAt: 1,
		lastActiveAt: 2,
	}
}

describe("session delete confirmation", () => {
	test("renders an irreversible delete confirmation with the session name", () => {
		const markup = renderToStaticMarkup(
			<SessionDeleteDialogBody
				agent={agent()}
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			hasTitle: markup.includes("Delete session"),
			hasSessionName: markup.includes("Greeting and Introduction"),
			hasIrreversibleCopy: markup.includes("cannot be undone"),
			hasDeleteAction: markup.includes("Delete"),
		}).toEqual({
			hasTitle: true,
			hasSessionName: true,
			hasIrreversibleCopy: true,
			hasDeleteAction: true,
		})
	})

	test("shows pending and error states", () => {
		const markup = renderToStaticMarkup(
			<SessionDeleteDialogBody
				agent={agent()}
				pending
				error="Failed to delete session"
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			hasPendingLabel: markup.includes("Deleting"),
			hasError: markup.includes("Failed to delete session"),
		}).toEqual({
			hasPendingLabel: true,
			hasError: true,
		})
	})

	test("navigates from a deleted active session to the same project new-chat route", () => {
		expect(
			deleteSessionNavigationTarget({
				deletedSessionId: "session-1",
				currentSessionId: "session-1",
				projectSlug: "devo-123",
			}),
		).toEqual({
			to: "/project/$projectSlug",
			params: { projectSlug: "devo-123" },
		})
	})

	test("does not navigate when a background session is deleted", () => {
		expect(
			deleteSessionNavigationTarget({
				deletedSessionId: "session-2",
				currentSessionId: "session-1",
				projectSlug: "devo-123",
			}),
		).toEqual(null)
	})
})
