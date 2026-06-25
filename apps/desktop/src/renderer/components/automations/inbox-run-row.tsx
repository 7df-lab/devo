/**
 * A single automation run row in the Completed/Archived sections.
 *
 * Shows: read/unread indicator, automation name, project, summary preview, time ago.
 * Right-click context menu: Mark read/unread, Archive, Open thread.
 */

import {
	ContextMenu,
	ContextMenuContent,
	ContextMenuItem,
	ContextMenuSeparator,
	ContextMenuTrigger,
} from "@devo/ui/components/context-menu"
import {
	ArchiveIcon,
	CheckCircle2Icon,
	CircleIcon,
	CopyIcon,
	ExternalLinkIcon,
	Loader2Icon,
} from "lucide-react"
import { memo, useCallback, useState } from "react"
import type { AutomationRun } from "../../../preload/api"
import { formatTimeAgo } from "../../lib/time-format"

interface InboxRunRowProps {
	run: AutomationRun
	automationName: string
	projectLabel: string | null
	isSelected: boolean
	onClick: () => void
	onArchive?: (runId: string) => void
	onMarkRead?: (runId: string) => void
}

export const InboxRunRow = memo(function InboxRunRow({
	run,
	automationName,
	projectLabel,
	isSelected,
	onClick,
	onArchive,
	onMarkRead,
}: InboxRunRowProps) {
	const [hovered, setHovered] = useState(false)

	const isUnread = run.readAt === null && run.status === "pending_review"
	const isArchived = run.status === "archived"
	const isRunning = run.status === "running" || run.status === "queued"
	const isAccepted = run.status === "accepted"

	const summary = run.resultSummary ?? run.resultTitle ?? null
	const timeText = run.createdAt ? formatTimeAgo(run.createdAt) : null

	const handleCopy = useCallback(
		(e: React.MouseEvent) => {
			e.stopPropagation()
			if (summary) {
				navigator.clipboard.writeText(summary)
			}
		},
		[summary],
	)

	const row = (
		<button
			type="button"
			onClick={onClick}
			onMouseEnter={() => setHovered(true)}
			onMouseLeave={() => setHovered(false)}
			className={`flex w-full items-start gap-2.5 rounded-md px-3 py-2 text-left transition-colors ${
				isSelected ? "bg-accent text-accent-foreground" : "hover:bg-accent/50"
			}`}
		>
			{/* Status indicator */}
			<div className="mt-1 shrink-0">
				{isRunning ? (
					<Loader2Icon className="size-4 animate-spin text-blue-500" />
				) : isUnread ? (
					<CircleIcon className="size-4 fill-blue-500 text-blue-500" />
				) : isArchived ? (
					<ArchiveIcon className="size-4 text-muted-foreground/60" />
				) : isAccepted ? (
					<CheckCircle2Icon className="size-4 text-muted-foreground/60" />
				) : (
					<CheckCircle2Icon className="size-4 text-muted-foreground/60" />
				)}
			</div>

			{/* Content */}
			<div className="min-w-0 flex-1">
				<div className="flex items-center gap-1.5">
					<span className="truncate text-sm font-medium">{automationName}</span>
					{projectLabel && (
						<span className="truncate text-xs text-muted-foreground">{projectLabel}</span>
					)}
				</div>
				{summary && <p className="mt-0.5 truncate text-xs text-muted-foreground">{summary}</p>}
			</div>

			{/* Right side: time or copy button */}
			<div className="mt-0.5 flex shrink-0 items-center gap-1">
				{hovered && !isArchived && summary ? (
					<button
						type="button"
						onClick={handleCopy}
						className="rounded p-0.5 text-muted-foreground hover:text-foreground"
					>
						<CopyIcon className="size-3.5" />
					</button>
				) : null}
				{timeText && <span className="text-xs tabular-nums text-muted-foreground">{timeText}</span>}
			</div>
		</button>
	)

	return (
		<ContextMenu>
			<ContextMenuTrigger render={row} />
			<ContextMenuContent>
				{onMarkRead && !isArchived && (
					<ContextMenuItem onSelect={() => onMarkRead(run.id)}>
						<CircleIcon className="size-4" />
						{isUnread ? "Mark as read" : "Mark as read"}
					</ContextMenuItem>
				)}
				{run.sessionId && (
					<ContextMenuItem onSelect={onClick}>
						<ExternalLinkIcon className="size-4" />
						Open thread
					</ContextMenuItem>
				)}
				{(onMarkRead || run.sessionId) && onArchive && !isArchived && <ContextMenuSeparator />}
				{onArchive && !isArchived && (
					<ContextMenuItem onSelect={() => onArchive(run.id)}>
						<ArchiveIcon className="size-4" />
						Archive
					</ContextMenuItem>
				)}
			</ContextMenuContent>
		</ContextMenu>
	)
})
