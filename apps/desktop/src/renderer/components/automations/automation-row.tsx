/**
 * A single automation config row in the "Scheduled" section.
 *
 * Shows: status icon, name (truncated), project path(s), "Starts in Xm" countdown.
 * Right-click context menu: Edit, Run Now, Pause/Resume, Delete.
 */

import {
	ContextMenu,
	ContextMenuContent,
	ContextMenuItem,
	ContextMenuSeparator,
	ContextMenuTrigger,
} from "@devo/ui/components/context-menu"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { CircleIcon, PauseIcon, PencilIcon } from "lucide-react"
import { memo, useRef } from "react"
import type { Automation } from "../../../preload/api"
import { useCountdown } from "../../hooks/use-countdown"

interface AutomationRowProps {
	automation: Automation
	isSelected: boolean
	onClick: () => void
	onEdit: (automation: Automation) => void
	onRunNow: (automationId: string) => void
	onTogglePause: (automation: Automation) => void
	onDelete: (automationId: string) => void
}

export const AutomationRow = memo(function AutomationRow({
	automation,
	isSelected,
	onClick,
	onEdit,
	onRunNow,
	onTogglePause,
	onDelete,
}: AutomationRowProps) {
	const rowButtonRef = useRef<HTMLButtonElement>(null)
	const menuReturnFocus = useRef<HTMLElement | null>(null)

	const StatusIcon =
		automation.status === "paused"
			? PauseIcon
			: automation.status === "active"
				? CircleIcon
				: CircleIcon

	const isPaused = automation.status === "paused"
	const countdownLabel = useCountdown(automation.nextRunAt)
	const countdownText = countdownLabel ? `Starts in ${countdownLabel}` : isPaused ? "Paused" : null

	const projectLabel =
		automation.workspaces.length > 0
			? automation.workspaces.map((w) => w.split("/").pop()).join(", ")
			: null

	const row = (
		<div
			onContextMenuCapture={(event) => {
				menuReturnFocus.current = (event.target as Element).closest("button") ?? rowButtonRef.current
			}}
			className={`group flex w-full items-center rounded-md text-sm transition-colors ${
				isSelected ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
			} ${isPaused ? "opacity-60" : ""}`}
		>
			<button
				ref={rowButtonRef}
				type="button"
				onClick={onClick}
				className="flex min-w-0 flex-1 items-center gap-2 rounded-md py-2 pl-3 pr-2 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
			>
				<StatusIcon aria-hidden="true" className={`size-4 shrink-0 ${automation.status === "active" ? "text-muted-foreground" : "text-muted-foreground/60"}`} />
				<div className="min-w-0 flex-1">
					<div className="flex items-center gap-1.5">
						<span className="truncate font-medium text-sm">{automation.name}</span>
						{projectLabel && <span className="truncate text-xs text-muted-foreground">{projectLabel}</span>}
					</div>
				</div>
				{countdownText && <span className="shrink-0 text-xs text-muted-foreground">{countdownText}</span>}
			</button>
			{/* A stable sibling slot stays reachable by keyboard without hiding the countdown. */}
			<div className="flex shrink-0 items-center py-2 pr-3">
				<Tooltip>
					<TooltipTrigger render={
						<button
							type="button"
							aria-label="Edit automation"
							onClick={() => onEdit(automation)}
							className="rounded p-0.5 text-muted-foreground opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring hover:text-foreground"
						/>
					}>
						<PencilIcon aria-hidden="true" className="size-3.5" />
					</TooltipTrigger>
					<TooltipContent>Edit automation</TooltipContent>
				</Tooltip>
			</div>
		</div>
	)

	return (
		<ContextMenu>
			<ContextMenuTrigger render={row} tabIndex={-1} />
			<ContextMenuContent finalFocus={menuReturnFocus}>
				<ContextMenuItem onClick={() => onEdit(automation)}>Edit</ContextMenuItem>
				<ContextMenuItem onClick={() => onRunNow(automation.id)}>Run now</ContextMenuItem>
				<ContextMenuItem onClick={() => onTogglePause(automation)}>
					{isPaused ? "Resume" : "Pause"}
				</ContextMenuItem>
				<ContextMenuSeparator />
				<ContextMenuItem className="text-destructive" onClick={() => onDelete(automation.id)}>
					Delete
				</ContextMenuItem>
			</ContextMenuContent>
		</ContextMenu>
	)
})
