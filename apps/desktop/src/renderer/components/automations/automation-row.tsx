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
import { memo, useState } from "react"
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
	const [hovered, setHovered] = useState(false)

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

	return (
		<ContextMenu>
			<ContextMenuTrigger
				render={
					<button
						type="button"
						onClick={onClick}
						onMouseEnter={() => setHovered(true)}
						onMouseLeave={() => setHovered(false)}
						className={`flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm transition-colors ${
							isSelected ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
						} ${isPaused ? "opacity-60" : ""}`}
					/>
				}
			>
				<StatusIcon
					className={`size-4 shrink-0 ${
						automation.status === "active" ? "text-muted-foreground" : "text-muted-foreground/60"
					}`}
				/>

				<div className="min-w-0 flex-1">
					<div className="flex items-center gap-1.5">
						<span className="truncate font-medium text-sm">{automation.name}</span>
						{projectLabel && (
							<span className="truncate text-xs text-muted-foreground">{projectLabel}</span>
						)}
					</div>
				</div>

				{hovered ? (
					<Tooltip>
						<TooltipTrigger
							render={
								<button
									type="button"
									className="shrink-0 rounded p-0.5 text-muted-foreground hover:text-foreground"
									onClick={(e) => {
										e.stopPropagation()
										onEdit(automation)
									}}
								/>
							}
						>
							<PencilIcon className="size-3.5" />
						</TooltipTrigger>
						<TooltipContent>Edit automation</TooltipContent>
					</Tooltip>
				) : (
					countdownText && (
						<span className="shrink-0 text-xs text-muted-foreground">{countdownText}</span>
					)
				)}
			</ContextMenuTrigger>

			<ContextMenuContent>
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
