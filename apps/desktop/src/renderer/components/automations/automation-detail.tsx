/**
 * Detail view for a selected automation config.
 *
 * Shows the automation's configuration summary (schedule, workspaces, execution
 * settings) and a filterable list of its past runs. Clicking a run navigates to
 * the nested run detail route.
 *
 * Rendered at /automations/:automationId
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Separator } from "@devo/ui/components/separator"
import { useNavigate, useParams } from "@tanstack/react-router"
import {
	CalendarIcon,
	CircleIcon,
	FolderIcon,
	Loader2Icon,
	PauseIcon,
	PencilIcon,
	PlayIcon,
	ZapIcon,
} from "lucide-react"
import { useCallback, useMemo, useState } from "react"
import { toast } from "sonner"
import type { Automation, AutomationRun } from "../../../preload/api"
import { useAutomationRuns, useAutomations } from "../../hooks/use-automations"
import { useCountdown } from "../../hooks/use-countdown"
import { formatScheduleSummary, rruleToScheduleConfig } from "../../lib/rrule-ui"
import { formatTimeAgo } from "../../lib/time-format"
import { runAutomationNow, updateAutomation } from "../../services/backend"
import { CreateAutomationDialog } from "./create-automation-dialog"

// ============================================================
// Status badge
// ============================================================

function RunStatusBadge({ status }: { status: AutomationRun["status"] }) {
	switch (status) {
		case "running":
		case "queued":
			return (
				<Badge variant="outline" className="gap-1 text-blue-600 dark:text-blue-400">
					<Loader2Icon className="size-3 animate-spin" />
					{status === "running" ? "Running" : "Queued"}
				</Badge>
			)
		case "pending_review":
			return (
				<Badge variant="outline" className="text-amber-600 dark:text-amber-400">
					Pending review
				</Badge>
			)
		case "accepted":
			return (
				<Badge variant="outline" className="text-green-600 dark:text-green-400">
					Accepted
				</Badge>
			)
		case "archived":
			return (
				<Badge variant="outline" className="text-muted-foreground">
					Archived
				</Badge>
			)
		case "failed":
			return (
				<Badge variant="destructive" className="text-xs">
					Failed
				</Badge>
			)
		default:
			return null
	}
}

// ============================================================
// Run row within automation detail
// ============================================================

function AutomationRunRow({ run, automationId }: { run: AutomationRun; automationId: string }) {
	const navigate = useNavigate()

	return (
		<button
			type="button"
			onClick={() =>
				navigate({
					to: "/automations/$automationId/runs/$runId",
					params: { automationId, runId: run.id },
				})
			}
			className="flex w-full items-center gap-3 rounded-md px-3 py-2 text-left text-sm transition-colors hover:bg-accent/50"
		>
			<div className="min-w-0 flex-1">
				<div className="flex items-center gap-2">
					<span className="truncate font-medium">{run.resultTitle ?? `Run #${run.attempt}`}</span>
					<RunStatusBadge status={run.status} />
				</div>
				{run.resultSummary && (
					<p className="mt-0.5 truncate text-xs text-muted-foreground">{run.resultSummary}</p>
				)}
			</div>
			<span className="shrink-0 text-xs tabular-nums text-muted-foreground">
				{formatTimeAgo(run.createdAt)}
			</span>
		</button>
	)
}

// ============================================================
// Main component
// ============================================================

export function AutomationDetail() {
	const { automationId } = useParams({ strict: false }) as { automationId: string }

	const automations = useAutomations()
	const allRuns = useAutomationRuns()
	const [editDialogOpen, setEditDialogOpen] = useState(false)

	const automation: Automation | null = useMemo(
		() => automations.find((a) => a.id === automationId) ?? null,
		[automations, automationId],
	)

	const countdownLabel = useCountdown(automation?.nextRunAt ?? null)

	const runs = useMemo(
		() =>
			allRuns
				.filter((r) => r.automationId === automationId)
				.sort((a, b) => b.createdAt - a.createdAt),
		[allRuns, automationId],
	)

	const scheduleSummary = useMemo(
		() =>
			automation ? formatScheduleSummary(rruleToScheduleConfig(automation.schedule.rrule)) : "",
		[automation],
	)

	const handleRunNow = useCallback(async () => {
		if (!automation) return
		try {
			await runAutomationNow(automation.id)
			toast.success("Automation run started", {
				description: "Check the inbox for results.",
			})
		} catch (err) {
			toast.error("Failed to run automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [automation])

	const handleTogglePause = useCallback(async () => {
		if (!automation) return
		try {
			await updateAutomation({
				id: automation.id,
				status: automation.status === "paused" ? "active" : "paused",
			})
		} catch (err) {
			toast.error("Failed to update automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [automation])

	if (!automation) {
		return (
			<div className="flex flex-1 items-center justify-center p-8">
				<p className="text-sm text-muted-foreground">Automation not found</p>
			</div>
		)
	}

	const isPaused = automation.status === "paused"
	const projectLabels = automation.workspaces.map((w) => w.split("/").pop() ?? w)

	return (
		<div className="flex h-full flex-col">
			{/* Header */}
			<div className="flex flex-col gap-2 border-b px-4 py-3">
				<div className="flex items-center gap-3">
					<div className="flex size-8 items-center justify-center rounded-md bg-muted">
						<ZapIcon className="size-4 text-muted-foreground" aria-hidden="true" />
					</div>
					<div className="min-w-0 flex-1">
						<h2 className="truncate text-sm font-semibold">{automation.name}</h2>
						<div className="flex items-center gap-2 text-xs text-muted-foreground">
							<span className="flex items-center gap-1">
								<CalendarIcon className="size-3" aria-hidden="true" />
								{scheduleSummary}
							</span>
							{isPaused ? (
								<>
									<CircleIcon className="size-1 fill-current" aria-hidden="true" />
									<span className="text-yellow-600 dark:text-yellow-400">Paused</span>
								</>
							) : countdownLabel ? (
								<>
									<CircleIcon className="size-1 fill-current" aria-hidden="true" />
									<span>Next in {countdownLabel}</span>
								</>
							) : null}
							{automation.runCount > 0 && (
								<>
									<CircleIcon className="size-1 fill-current" aria-hidden="true" />
									<span>
										{automation.runCount} run{automation.runCount !== 1 ? "s" : ""}
									</span>
								</>
							)}
						</div>
					</div>
					<Button variant="ghost" size="sm" onClick={() => setEditDialogOpen(true)}>
						<PencilIcon className="size-4" />
					</Button>
				</div>

				{/* Action buttons */}
				<div className="flex items-center gap-2">
					<Button
						variant="outline"
						size="sm"
						className="h-7 gap-1.5 text-xs"
						onClick={handleTogglePause}
					>
						{isPaused ? <PlayIcon className="size-3.5" /> : <PauseIcon className="size-3.5" />}
						{isPaused ? "Resume" : "Pause"}
					</Button>
					<Button
						variant="outline"
						size="sm"
						className="h-7 gap-1.5 text-xs"
						onClick={handleRunNow}
					>
						<ZapIcon className="size-3.5" />
						Run now
					</Button>
				</div>
			</div>

			{/* Workspaces */}
			{projectLabels.length > 0 && (
				<div className="flex items-center gap-2 border-b px-4 py-2">
					<FolderIcon className="size-3.5 text-muted-foreground" aria-hidden="true" />
					<div className="flex flex-wrap gap-1">
						{projectLabels.map((label) => (
							<Badge key={label} variant="secondary" className="text-xs">
								{label}
							</Badge>
						))}
					</div>
				</div>
			)}

			<Separator />

			{/* Run history */}
			<div className="flex-1 overflow-y-auto">
				{runs.length === 0 ? (
					<div className="flex flex-1 items-center justify-center p-8">
						<p className="text-sm text-muted-foreground">No runs yet</p>
					</div>
				) : (
					<div className="p-1">
						<div className="px-3 py-1.5">
							<span className="text-xs font-medium text-muted-foreground">Run history</span>
						</div>
						{runs.map((run) => (
							<AutomationRunRow key={run.id} run={run} automationId={automationId} />
						))}
					</div>
				)}
			</div>

			{/* Edit dialog */}
			<CreateAutomationDialog
				open={editDialogOpen}
				onOpenChange={setEditDialogOpen}
				editAutomation={automation}
			/>
		</div>
	)
}
