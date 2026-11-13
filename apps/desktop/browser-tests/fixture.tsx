import React, { useState } from "react"
import { createRoot } from "react-dom/client"
import { Toaster } from "sonner"
import { InboxRunRow } from "../src/renderer/components/automations/inbox-run-row"
import type { AutomationRun } from "../src/preload/api"
import "../src/renderer/index.css"
const base: AutomationRun = {
	id: "run",
	automationId: "automation",
	workspace: "/workspace",
	status: "pending_review",
	attempt: 1,
	sessionId: "session",
	worktreePath: null,
	startedAt: null,
	completedAt: null,
	timeoutAt: null,
	resultTitle: null,
	resultSummary:
		"A long automation summary that should truncate without pushing actions outside the window.",
	resultHasActionable: true,
	resultBranch: null,
	resultPrUrl: null,
	errorMessage: null,
	archivedReason: null,
	archivedAssistantMessage: null,
	readAt: null,
	createdAt: Date.now(),
	updatedAt: Date.now(),
}
function Fixture() {
	const [opened, setOpened] = useState(0)
	const [marked, setMarked] = useState(0)
	const [archived, setArchived] = useState(0)
	const variant = new URLSearchParams(location.search).get("variant")
	const run = {
		...base,
		...(variant === "archived" ? { status: "archived" as const } : {}),
		...(variant === "empty" ? { resultSummary: null } : {}),
	}
	return (
		<main className="w-full max-w-xl p-2 bg-background text-foreground">
			<InboxRunRow
				run={run}
				automationName="Daily review with a long descriptive title"
				projectLabel="Devo"
				isSelected={false}
				onClick={() => setOpened((n) => n + 1)}
				onMarkRead={() => setMarked((n) => n + 1)}
				onArchive={() => setArchived((n) => n + 1)}
			/>
			<output aria-label="Opened">{opened}</output>
			<output aria-label="Marked">{marked}</output>
			<output aria-label="Archived">{archived}</output>
			<Toaster />
		</main>
	)
}
createRoot(document.getElementById("root")!).render(<Fixture />)
