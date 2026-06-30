import {
	ChevronRightIcon,
	CirclePauseIcon,
	CirclePlayIcon,
	GoalIcon,
	Loader2Icon,
	PencilIcon,
	Trash2Icon,
} from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"
import { cn } from "@devo/ui/lib/utils"
import { formatWorkDuration } from "../../lib/session-metrics"

export type ComposerGoalStatus = "active" | "paused" | "budgetLimited" | "complete"

export interface ComposerGoal {
	objective: string
	status: ComposerGoalStatus
	timeUsedSeconds?: number | string | bigint | null
	observedAtMs?: number
}

interface ComposerStatusStackProps {
	goal?: ComposerGoal | null
	goalAction?: "edit" | "pause" | "resume" | "clear" | null
	onEditGoal?: () => void
	onPauseGoal?: () => void
	onResumeGoal?: () => void
	onClearGoal?: () => void
}

function protocolNumber(value: number | string | bigint | null | undefined): number {
	if (typeof value === "number") return Number.isFinite(value) ? value : 0
	if (typeof value === "bigint") return Number(value)
	if (typeof value === "string") {
		const parsed = Number(value)
		return Number.isFinite(parsed) ? parsed : 0
	}
	return 0
}

function goalStatusLabel(status: ComposerGoalStatus): string {
	switch (status) {
		case "active":
			return "Pursuing goal"
		case "paused":
			return "Goal paused"
		case "budgetLimited":
			return "Goal budget reached"
		case "complete":
			return "Goal complete"
	}
}

function GoalElapsed({ goal }: { goal: ComposerGoal }) {
	const [now, setNow] = useState(() => Date.now())

	useEffect(() => {
		if (goal.status !== "active") return
		const timer = setInterval(() => setNow(Date.now()), 1_000)
		return () => clearInterval(timer)
	}, [goal.status])

	const observedAt = goal.observedAtMs ?? now
	const liveDeltaMs = goal.status === "active" ? Math.max(0, now - observedAt) : 0
	const elapsedMs = protocolNumber(goal.timeUsedSeconds) * 1_000 + liveDeltaMs
	if (elapsedMs < 1_000) return null

	return (
		<span className="shrink-0 tabular-nums text-muted-foreground/80">
			{formatWorkDuration(elapsedMs)}
		</span>
	)
}

function RowIconButton({
	label,
	disabled,
	active,
	onClick,
	children,
}: {
	label: string
	disabled?: boolean
	active?: boolean
	onClick?: () => void
	children: ReactNode
}) {
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			disabled={disabled}
			onClick={onClick}
			className={cn(
				"grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
				active && "bg-muted text-foreground",
			)}
		>
			{children}
		</button>
	)
}

interface ActiveGoalRowProps {
	goal: ComposerGoal
	goalAction?: ComposerStatusStackProps["goalAction"]
	onEditGoal?: () => void
	onPauseGoal?: () => void
	onResumeGoal?: () => void
	onClearGoal?: () => void
}

function ActiveGoalRow({
	goal,
	goalAction = null,
	onEditGoal,
	onPauseGoal,
	onResumeGoal,
	onClearGoal,
}: ActiveGoalRowProps) {
	const statusLabel = goalStatusLabel(goal.status)
	const isPaused = goal.status === "paused" || goal.status === "budgetLimited"
	const toggleLabel = isPaused ? "Resume goal" : "Pause goal"
	const toggleAction = isPaused ? onResumeGoal : onPauseGoal

	return (
		<div className="flex min-h-10 items-center gap-2 px-3 text-sm text-muted-foreground">
			<GoalIcon className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/75" />
			<div className="min-w-0 flex flex-1 items-center gap-1.5">
				<span className="shrink-0 font-medium text-foreground">{statusLabel}</span>
				<span className="truncate">{goal.objective}</span>
			</div>
			<GoalElapsed goal={goal} />
			<div className="flex shrink-0 items-center gap-0.5">
				<RowIconButton
					label="Edit goal"
					disabled={goalAction !== null}
					active={goalAction === "edit"}
					onClick={onEditGoal}
				>
					{goalAction === "edit" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : (
						<PencilIcon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
				<RowIconButton
					label={toggleLabel}
					disabled={goalAction !== null}
					active={goalAction === "pause" || goalAction === "resume"}
					onClick={toggleAction}
				>
					{goalAction === "pause" || goalAction === "resume" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : isPaused ? (
						<CirclePlayIcon className="size-3.5 stroke-[1.5]" />
					) : (
						<CirclePauseIcon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
				<RowIconButton
					label="Clear goal"
					disabled={goalAction !== null}
					active={goalAction === "clear"}
					onClick={onClearGoal}
				>
					{goalAction === "clear" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : (
						<Trash2Icon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
				<RowIconButton label="Goal details" disabled>
					<ChevronRightIcon className="size-3.5 stroke-[1.5]" />
				</RowIconButton>
			</div>
		</div>
	)
}

export function ComposerStatusStack({
	goal,
	goalAction = null,
	onEditGoal,
	onPauseGoal,
	onResumeGoal,
	onClearGoal,
}: ComposerStatusStackProps) {
	if (!goal) return null

	return (
		// User requirement: reuse this composer-adjacent strip for goal state
		// and future queued follow-up rows instead of scattering status below messages.
		<div className="mb-0 overflow-hidden rounded-t-[20px] border border-b-0 border-border/70 bg-background/95 shadow-[0_10px_34px_rgba(0,0,0,0.06)]">
			<ActiveGoalRow
				goal={goal}
				goalAction={goalAction}
				onEditGoal={onEditGoal}
				onPauseGoal={onPauseGoal}
				onResumeGoal={onResumeGoal}
				onClearGoal={onClearGoal}
			/>
		</div>
	)
}
