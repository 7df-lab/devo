/**
 * Empty state for the right panel when no automation run is selected.
 *
 * Shows unread count if there are unread runs, or a create CTA if no automations exist.
 */

import { Button } from "@devo/ui/components/button"
import { PlusIcon, SmileIcon, ZapIcon } from "lucide-react"
import { useAutomations, useUnreadRunCount } from "../../hooks/use-automations"

interface InboxEmptyStateProps {
	onNewClick?: () => void
}

export function InboxEmptyState({ onNewClick }: InboxEmptyStateProps) {
	const automations = useAutomations()
	const unreadCount = useUnreadRunCount()

	const hasAutomations = automations.length > 0

	return (
		<div className="flex flex-1 flex-col items-center justify-center gap-4 p-8">
			<div className="flex size-16 items-center justify-center rounded-2xl bg-muted">
				{hasAutomations ? (
					<SmileIcon className="size-8 text-muted-foreground" />
				) : (
					<ZapIcon className="size-8 text-muted-foreground" />
				)}
			</div>

			<div className="space-y-1 text-center">
				{hasAutomations ? (
					<p className="text-sm text-muted-foreground">
						{unreadCount > 0
							? `${unreadCount} unread automation${unreadCount === 1 ? "" : "s"}`
							: "No unread automations"}
					</p>
				) : (
					<>
						<h2 className="text-lg font-semibold">Automations</h2>
						<p className="text-sm text-muted-foreground">
							Set up recurring AI tasks that run on a schedule.
						</p>
						{onNewClick && (
							<Button onClick={onNewClick} size="sm" className="mt-3">
								<PlusIcon className="size-4" />
								Create Automation
							</Button>
						)}
					</>
				)}
			</div>
		</div>
	)
}
