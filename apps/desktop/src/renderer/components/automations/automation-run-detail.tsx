/**
 * Detail view for a selected automation run.
 *
 * Renders the exact same session UI as normal chat sessions when the run has
 * a real sessionId. For runs that are still starting up (no session yet),
 * polls the backend at a short interval until the sessionId appears so the
 * user can watch the session live.
 *
 * Marks runs as read on mount when they are unread.
 */

import { MessageResponse } from "@devo/ui/components/ai-elements/message"
import { useParams } from "@tanstack/react-router"
import { useSetAtom } from "jotai"
import { InboxIcon, Loader2Icon } from "lucide-react"
import { useEffect, useMemo, useRef } from "react"
import type { Automation, AutomationRun } from "../../../preload/api"
import { markRunReadLocalAtom, setAutomationRunsAtom } from "../../atoms/automations"
import { useAutomationRuns, useAutomations } from "../../hooks/use-automations"
import { formatDuration } from "../../lib/time-format"
import { fetchAutomationRuns, markAutomationRunRead } from "../../services/backend"
import { SessionView } from "../session-view"

/** Poll interval when waiting for a running run to get its sessionId. */
const SESSION_POLL_INTERVAL = 2_000

export function AutomationRunDetail() {
	const { runId } = useParams({ strict: false }) as { runId: string }

	const runs = useAutomationRuns()
	const automations = useAutomations()
	const markReadLocal = useSetAtom(markRunReadLocalAtom)
	const setRuns = useSetAtom(setAutomationRunsAtom)

	const run: AutomationRun | null = useMemo(
		() => runs.find((r) => r.id === runId) ?? null,
		[runs, runId],
	)

	const automation: Automation | null = useMemo(
		() => (run ? (automations.find((a) => a.id === run.automationId) ?? null) : null),
		[automations, run],
	)

	// Mark as read on mount
	useEffect(() => {
		if (run && run.readAt === null && run.status === "pending_review") {
			markReadLocal(run.id)
			markAutomationRunRead(run.id).catch(() => {
				// Silently ignore -- the next poll will correct state
			})
		}
	}, [run, markReadLocal])

	// --- Short-interval poll for sessionId while the run is starting up ---
	// When a run is "running" but has no sessionId yet, the session is being
	// created in the background (worktree + session creation). Poll every 2s
	// so we can switch to the live SessionView as soon as it's available.
	const needsSessionPoll = run !== null && run.status === "running" && !run.sessionId
	const mountedRef = useRef(true)

	useEffect(() => {
		mountedRef.current = true
		return () => {
			mountedRef.current = false
		}
	}, [])

	useEffect(() => {
		if (!needsSessionPoll) return

		const timer = setInterval(async () => {
			try {
				const freshRuns = await fetchAutomationRuns()
				if (mountedRef.current) {
					setRuns(freshRuns)
				}
			} catch {
				// Ignore fetch errors during polling
			}
		}, SESSION_POLL_INTERVAL)

		return () => clearInterval(timer)
	}, [needsSessionPoll, setRuns])

	if (!run) {
		return (
			<div className="flex flex-1 items-center justify-center p-8">
				<p className="text-sm text-muted-foreground">Run not found</p>
			</div>
		)
	}

	// Runs with a real session get the exact same UI as normal chat sessions
	if (run.sessionId) {
		return <SessionView sessionId={run.sessionId} />
	}

	// Stub runs (no session yet) get a simple summary view
	return <RunStubView run={run} automationName={automation?.name ?? "Automation"} />
}

// ============================================================
// Stub view for runs without a real session
// ============================================================

function RunStubView({ run, automationName }: { run: AutomationRun; automationName: string }) {
	const duration = run.startedAt && run.completedAt ? run.completedAt - run.startedAt : null
	const isStartingUp = run.status === "running" && !run.sessionId

	return (
		<div className="flex h-full flex-col items-center justify-center gap-4 p-8 text-center">
			<div className="flex size-12 items-center justify-center rounded-full bg-muted">
				{isStartingUp ? (
					<Loader2Icon className="size-6 animate-spin text-muted-foreground" aria-hidden="true" />
				) : (
					<InboxIcon className="size-6 text-muted-foreground" aria-hidden="true" />
				)}
			</div>

			<div className="max-w-md space-y-2">
				<h3 className="text-sm font-semibold">{run.resultTitle ?? automationName}</h3>

				{duration !== null && duration > 0 && (
					<p className="text-xs text-muted-foreground">Ran for {formatDuration(duration)}</p>
				)}

				{run.resultSummary ? (
					<div className="prose prose-sm dark:prose-invert mx-auto max-w-none text-left">
						<MessageResponse>{run.resultSummary}</MessageResponse>
					</div>
				) : isStartingUp ? (
					<p className="text-sm text-muted-foreground">
						Starting session... The live view will appear momentarily.
					</p>
				) : run.status === "queued" ? (
					<p className="text-sm text-muted-foreground">This run is queued and waiting to start.</p>
				) : run.status === "running" ? (
					<p className="text-sm text-muted-foreground">This run is in progress...</p>
				) : (
					<p className="text-sm text-muted-foreground">No output recorded for this run.</p>
				)}

				{run.errorMessage && <p className="text-sm text-destructive">{run.errorMessage}</p>}
			</div>
		</div>
	)
}
