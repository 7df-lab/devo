/**
 * Scrollable list with sticky section headers: "Scheduled", "Completed", "Archived".
 *
 * Groups automation configs (Scheduled) and runs (Completed/Archived) into sections.
 */

import { useNavigate, useParams } from "@tanstack/react-router"
import { useSetAtom } from "jotai"
import { useCallback, useMemo } from "react"
import type { Automation, AutomationRun } from "../../../preload/api"
import { archiveRunLocalAtom, markRunReadLocalAtom } from "../../atoms/automations"
import { useAutomationRuns, useAutomations } from "../../hooks/use-automations"
import { archiveAutomationRun, markAutomationRunRead } from "../../services/backend"
import { AutomationRow } from "./automation-row"
import { InboxRunRow } from "./inbox-run-row"

interface InboxRunListProps {
	onEditAutomation: (automation: Automation) => void
	onRunNow: (automationId: string) => void
	onTogglePause: (automation: Automation) => void
	onDeleteAutomation: (automationId: string) => void
}

// ============================================================
// Helpers
// ============================================================

function getRunProjectLabel(run: AutomationRun): string | null {
	if (!run.workspace) return null
	return run.workspace.split("/").pop() ?? null
}

// ============================================================
// Section header
// ============================================================

function SectionHeader({ label }: { label: string }) {
	return (
		<div className="sticky top-0 z-10 bg-background/95 px-4 py-1.5 backdrop-blur-sm">
			<span className="text-xs font-medium text-muted-foreground">{label}</span>
		</div>
	)
}

// ============================================================
// Main component
// ============================================================

export function InboxRunList({
	onEditAutomation,
	onRunNow,
	onTogglePause,
	onDeleteAutomation,
}: InboxRunListProps) {
	const automations = useAutomations()
	const runs = useAutomationRuns()
	const navigate = useNavigate()
	const params = useParams({ strict: false }) as {
		automationId?: string
		runId?: string
	}
	const selectedAutomationId = params.automationId ?? null
	const selectedRunId = params.runId ?? null
	const archiveLocal = useSetAtom(archiveRunLocalAtom)
	const markReadLocal = useSetAtom(markRunReadLocalAtom)

	const handleArchive = useCallback(
		(runId: string) => {
			archiveLocal(runId)
			archiveAutomationRun(runId).catch(() => {})
		},
		[archiveLocal],
	)

	const handleMarkRead = useCallback(
		(runId: string) => {
			markReadLocal(runId)
			markAutomationRunRead(runId).catch(() => {})
		},
		[markReadLocal],
	)

	// Build an automation name lookup
	const automationMap = useMemo(() => {
		const map = new Map<string, Automation>()
		for (const a of automations) {
			map.set(a.id, a)
		}
		return map
	}, [automations])

	// Group runs by status
	const { scheduled, completed, archived } = useMemo(() => {
		const completedRuns: AutomationRun[] = []
		const archivedRuns: AutomationRun[] = []

		for (const run of runs) {
			if (run.status === "archived") {
				archivedRuns.push(run)
			} else {
				completedRuns.push(run)
			}
		}

		// Sort completed by created date descending
		completedRuns.sort((a, b) => b.createdAt - a.createdAt)
		archivedRuns.sort((a, b) => b.createdAt - a.createdAt)

		// Scheduled = active automations sorted by next run time
		const scheduledAutomations = automations
			.filter((a) => a.status === "active" || a.status === "paused")
			.sort((a, b) => (a.nextRunAt ?? Infinity) - (b.nextRunAt ?? Infinity))

		return {
			scheduled: scheduledAutomations,
			completed: completedRuns,
			archived: archivedRuns,
		}
	}, [automations, runs])

	const isEmpty = scheduled.length === 0 && completed.length === 0 && archived.length === 0

	if (isEmpty) {
		return (
			<div className="flex flex-1 items-center justify-center p-4">
				<p className="text-sm text-muted-foreground">No automations yet</p>
			</div>
		)
	}

	return (
		<div className="flex-1 overflow-y-auto">
			{/* Scheduled section */}
			{scheduled.length > 0 && (
				<div>
					<SectionHeader label="Scheduled" />
					<div className="px-1 pb-1">
						{scheduled.map((automation) => (
							<AutomationRow
								key={automation.id}
								automation={automation}
								isSelected={automation.id === selectedAutomationId && !selectedRunId}
								onClick={() =>
									navigate({
										to: "/automations/$automationId",
										params: { automationId: automation.id },
									})
								}
								onEdit={onEditAutomation}
								onRunNow={onRunNow}
								onTogglePause={onTogglePause}
								onDelete={onDeleteAutomation}
							/>
						))}
					</div>
				</div>
			)}

			{/* Completed section */}
			{completed.length > 0 && (
				<div>
					<SectionHeader label="Completed" />
					<div className="px-1 pb-1">
						{completed.map((run) => {
							const automation = automationMap.get(run.automationId)
							return (
								<InboxRunRow
									key={run.id}
									run={run}
									automationName={automation?.name ?? "Unknown"}
									projectLabel={getRunProjectLabel(run)}
									isSelected={run.id === selectedRunId}
									onClick={() =>
										navigate({
											to: "/automations/$automationId/runs/$runId",
											params: { automationId: run.automationId, runId: run.id },
										})
									}
									onArchive={handleArchive}
									onMarkRead={handleMarkRead}
								/>
							)
						})}
					</div>
				</div>
			)}

			{/* Archived section */}
			{archived.length > 0 && (
				<div>
					<SectionHeader label="Archived" />
					<div className="px-1 pb-1">
						{archived.map((run) => {
							const automation = automationMap.get(run.automationId)
							return (
								<InboxRunRow
									key={run.id}
									run={run}
									automationName={automation?.name ?? "Unknown"}
									projectLabel={getRunProjectLabel(run)}
									isSelected={run.id === selectedRunId}
									onClick={() =>
										navigate({
											to: "/automations/$automationId/runs/$runId",
											params: { automationId: run.automationId, runId: run.id },
										})
									}
									onArchive={handleArchive}
									onMarkRead={handleMarkRead}
								/>
							)
						})}
					</div>
				</div>
			)}
		</div>
	)
}
