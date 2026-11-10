import {
	Popover,
	PopoverContent,
	PopoverDescription,
	PopoverTitle,
	PopoverTrigger,
} from "@devo/ui/components/popover"
import type { NativeSession } from "@devo-ai/sdk/v2/client"
import { useNavigate } from "@tanstack/react-router"
import { ArrowUpRightIcon, GitBranchIcon, RefreshCwIcon, SquareIcon, XIcon } from "lucide-react"
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
	const [scope, setScope] = useState<"all" | "attention">("all")
	const navigate = useNavigate()
	const roster = useSubagentRoster(agent.sessionId, agent.directory, open)
	const attention = roster.sessions.filter(
		(session) =>
			session.flags.includes("waitingApproval") || session.flags.includes("waitingUserInput"),
	)
	const visibleSessions = scope === "attention" ? attention : roster.sessions
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
					Direct children of this session. Updates automatically while open.
				</PopoverDescription>
				<fieldset
					className="m-0 flex min-w-0 flex-wrap gap-1 border-0 p-0"
					aria-label="Filter subagents"
				>
					{(["all", "attention"] as const).map((value) => (
						<button
							key={value}
							type="button"
							aria-pressed={scope === value}
							onClick={() => setScope(value)}
							className={`rounded-md px-2.5 py-1 text-xs transition-colors focus-visible:outline-2 focus-visible:outline-ring ${scope === value ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-muted/60"}`}
						>
							{value === "attention"
								? `Needs attention (${attention.length})`
								: `All (${roster.sessions.length})`}
						</button>
					))}
				</fieldset>
				<p className="text-[11px] text-muted-foreground">
					Filters and counts cover loaded children. Needs attention means waiting for approval or
					input.
				</p>
				{roster.error && (
					<p role="alert" className="text-xs text-destructive">
						{roster.error} {roster.sessions.length > 0 && "Showing the last available state."}
					</p>
				)}
				{roster.loading && (
					<output className="text-xs text-muted-foreground">Checking subagents…</output>
				)}
				{!roster.loading && !roster.error && visibleSessions.length === 0 && (
					<p className="py-3 text-xs text-muted-foreground">
						{scope === "attention"
							? "No loaded subagents are waiting for approval or input."
							: roster.nextCursor
								? "No subagents found in the checked pages yet."
								: "No saved subagents in this session."}
					</p>
				)}
				<ul aria-label="Subagent sessions" className="max-h-72 overflow-y-auto">
					{visibleSessions.map((session) => {
						const stop = roster.stopState(session.id)
						return (
							<li key={session.id}>
								<div className="flex items-center gap-1">
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
										className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-2 text-left text-xs transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring"
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
									{session.status === "active" && !session.archived && (
										<button
											type="button"
											aria-label={`Stop ${session.title || "Untitled subagent"}`}
											title="Interrupt current work. Queued messages may still run."
											disabled={!roster.canStop || stop?.status === "pending"}
											onClick={() => void roster.stop(session.id)}
											className="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
										>
											<SquareIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
										</button>
									)}
								</div>
								{stop?.status === "error" && (
									<p role="alert" className="px-2 pb-2 text-xs text-destructive">
										{stop.message}
									</p>
								)}
								{stop?.status === "pending" && (
									<output className="block px-2 pb-2 text-xs text-muted-foreground">
										Requesting stop…
									</output>
								)}
							</li>
						)
					})}
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
					prompt. Stop interrupts current work. Queued messages may still run.
				</p>
			</PopoverContent>
		</Popover>
	)
}
