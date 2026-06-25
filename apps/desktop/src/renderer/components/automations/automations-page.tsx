/**
 * Automations inbox page -- two-panel layout with run list (left) and detail (right).
 *
 * This is the main view rendered at /automations. It uses a ResizablePanelGroup
 * for the split layout and drives the data pipeline via useAutomationData().
 */

import { Alert, AlertAction, AlertDescription } from "@devo/ui/components/alert"
import {
	ResizableHandle,
	ResizablePanel,
	ResizablePanelGroup,
} from "@devo/ui/components/resizable"
import { Outlet } from "@tanstack/react-router"
import { useAtom } from "jotai"
import { XIcon } from "lucide-react"
import { useCallback, useState } from "react"
import { toast } from "sonner"
import type { Automation } from "../../../preload/api"
import { automationsBannerDismissedAtom } from "../../atoms/preferences"
import { useAutomationData } from "../../hooks/use-automation-data"
import { deleteAutomation, runAutomationNow, updateAutomation } from "../../services/backend"
import { CreateAutomationDialog } from "./create-automation-dialog"
import { InboxRunList } from "./inbox-run-list"
import { InboxToolbar } from "./inbox-toolbar"

// ============================================================
// Main page component
// ============================================================

export function AutomationsPage() {
	// Single data gateway -- fetches automations + runs, subscribes to updates
	useAutomationData()

	const [dialogOpen, setDialogOpen] = useState(false)
	const [editingAutomation, setEditingAutomation] = useState<Automation | null>(null)

	const handleNewClick = useCallback(() => {
		setEditingAutomation(null)
		setDialogOpen(true)
	}, [])

	const handleEditAutomation = useCallback((automation: Automation) => {
		setEditingAutomation(automation)
		setDialogOpen(true)
	}, [])

	const handleRunNow = useCallback(async (automationId: string) => {
		try {
			await runAutomationNow(automationId)
			toast.success("Automation run started", {
				description: "Check the inbox for results.",
			})
		} catch (err) {
			toast.error("Failed to run automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [])

	const handleTogglePause = useCallback(async (automation: Automation) => {
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
	}, [])

	const handleDeleteAutomation = useCallback(async (automationId: string) => {
		try {
			await deleteAutomation(automationId)
		} catch (err) {
			toast.error("Failed to delete automation", {
				description: err instanceof Error ? err.message : undefined,
			})
		}
	}, [])

	const handleDialogChange = useCallback((open: boolean) => {
		setDialogOpen(open)
		if (!open) setEditingAutomation(null)
	}, [])

	const [bannerDismissed, setBannerDismissed] = useAtom(automationsBannerDismissedAtom)

	return (
		<div className="flex h-full flex-col">
			{!bannerDismissed && (
				<div className="border-b border-border/50 px-4 py-3">
					<Alert className="border-amber-400/40 bg-amber-50 text-amber-900 dark:border-amber-500/30 dark:bg-amber-500/5 dark:text-amber-200/90 [&>svg]:text-amber-600 dark:[&>svg]:text-amber-400">
						<AlertDescription className="text-[12px] text-amber-800 dark:text-amber-200/70">
							<span className="font-medium text-amber-900 dark:text-amber-200/90">Automations run unattended</span>
							{" "}with broad permissions: all tools are allowed (file reads, edits, bash
							commands) and interactive prompts are auto-denied since no one is watching.
						</AlertDescription>
						<AlertAction>
							<button
								type="button"
								onClick={() => setBannerDismissed(true)}
								className="rounded p-1 text-amber-600/50 transition-colors hover:text-amber-800 dark:text-amber-200/40 dark:hover:text-amber-200/80"
								aria-label="Dismiss"
							>
							<XIcon className="size-3.5" aria-hidden="true" />
							</button>
						</AlertAction>
					</Alert>
				</div>
			)}
			<ResizablePanelGroup orientation="horizontal" className="flex-1" id="automations-inbox">
				{/* Left panel: list */}
				<ResizablePanel id="automations-list" defaultSize="35%" minSize="25%" maxSize="50%">
					<div className="flex h-full flex-col">
						<InboxToolbar onNewClick={handleNewClick} />
						<InboxRunList
							onEditAutomation={handleEditAutomation}
							onRunNow={handleRunNow}
							onTogglePause={handleTogglePause}
							onDeleteAutomation={handleDeleteAutomation}
						/>
					</div>
				</ResizablePanel>

				<ResizableHandle />

				{/* Right panel: detail or empty state */}
				<ResizablePanel id="automations-detail" defaultSize="65%" minSize="40%">
					<div className="flex h-full flex-col">
						<Outlet />
					</div>
				</ResizablePanel>
			</ResizablePanelGroup>

			<CreateAutomationDialog
				open={dialogOpen}
				onOpenChange={handleDialogChange}
				editAutomation={editingAutomation}
			/>
		</div>
	)
}
