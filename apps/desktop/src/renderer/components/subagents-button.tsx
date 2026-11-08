import {
	Popover,
	PopoverContent,
	PopoverDescription,
	PopoverTitle,
	PopoverTrigger,
} from "@devo/ui/components/popover"
import type { NativeSession } from "@devo-ai/sdk/v2/client"
import { useNavigate } from "@tanstack/react-router"
import { ArrowUpRightIcon, GitBranchIcon, RefreshCwIcon, XIcon } from "lucide-react"
import { useState } from "react"
import { useSubagentRoster } from "../hooks/use-subagent-roster"
import type { Agent } from "../lib/types"

/** Native idle means no active turn, not successful task completion. */
export function subagentStatus(session: NativeSession): string {
	if (session.flags.includes("waitingApproval")) return "Needs approval"
	if (session.flags.includes("waitingUserInput")) return "Needs input"
	if (session.flags.includes("compacting")) return "Compacting"
	if (session.flags.includes("updatingGoal")) return "Updating goal"
	if (session.status === "active") return "Working"
	if (session.queuedCount > 0) return "Queued"
	return "Idle"
}

export function SubagentsButton({ agent }: { agent: Agent }) {
	const [open, setOpen] = useState(false)
	const navigate = useNavigate()
	const roster = useSubagentRoster(agent.sessionId, agent.directory, open)
	return (
		<Popover open={open} onOpenChange={setOpen}>
			<PopoverTrigger
				aria-label="Subagents"
				title="Subagents"
				className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground data-popup-open:bg-muted"
			>
				<GitBranchIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
			</PopoverTrigger>
			<PopoverContent align="end" className="w-80 max-w-[calc(100vw-2rem)] gap-2 p-3">
				<div className="flex items-center gap-2">
					<PopoverTitle className="min-w-0 flex-1 text-[13px]">Subagents</PopoverTitle>
					<button
						type="button"
						aria-label="Refresh subagents"
						disabled={roster.loading}
						onClick={() => void roster.refresh()}
						className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted disabled:opacity-50"
					>
						<RefreshCwIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
					</button>
					<button
						type="button"
						aria-label="Close subagents"
						onClick={() => setOpen(false)}
						className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted"
					>
						<XIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
					</button>
				</div>
				<PopoverDescription className="text-xs">
					Direct children of this session. Refresh to check for changes.
				</PopoverDescription>
				{roster.error && (
					<p role="alert" className="text-xs text-destructive">
						{roster.error} {roster.sessions.length > 0 && "Showing the previous snapshot."}
					</p>
				)}
				{roster.loading && (
					<output className="text-xs text-muted-foreground">Checking subagents…</output>
				)}
				{!roster.loading && !roster.error && roster.sessions.length === 0 && (
					<p className="py-3 text-xs text-muted-foreground">
						{roster.nextCursor
							? "No subagents found in the checked pages yet."
							: "No saved subagents in this session."}
					</p>
				)}
				<ul aria-label="Subagent sessions" className="max-h-72 overflow-y-auto">
					{roster.sessions.map((session) => (
						<li key={session.id}>
							<button
								type="button"
								onClick={() => {
									setOpen(false)
									void navigate({
										to: "/project/$projectSlug/session/$sessionId",
										params: {
											projectSlug: agent.projectSlug,
											sessionId: session.id,
										},
									})
								}}
								className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-xs transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"
							>
								<span className="min-w-0 flex-1">
									<span className="block truncate font-medium">
										{session.title || "Untitled subagent"}
									</span>
									<span className="block text-muted-foreground">
										{subagentStatus(session)}
										{session.archived ? " · Archived" : ""}
									</span>
								</span>
								<ArrowUpRightIcon
									className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"
									aria-hidden="true"
								/>
							</button>
						</li>
					))}
				</ul>
				{roster.nextCursor && (
					<button
						type="button"
						disabled={roster.loading}
						onClick={() => void roster.loadMore()}
						className="rounded-md border border-border px-2 py-1.5 text-xs disabled:opacity-50"
					>
						Check more sessions ({roster.scanned} checked)
					</button>
				)}
				<p className="text-[11px] text-muted-foreground">
					Idle does not confirm completion. Open a child to view its conversation and any recovery
					prompt.
				</p>
			</PopoverContent>
		</Popover>
	)
}
