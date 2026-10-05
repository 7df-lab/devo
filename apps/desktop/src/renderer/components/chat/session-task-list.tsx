import { nativeItemType } from "@devo-ai/sdk/v2/client"
import { cn } from "@devo/ui/lib/utils"
import { useAtomValue } from "jotai"
import {
	CheckCircle2Icon,
	ChevronDownIcon,
	ChevronUpIcon,
	CircleDotIcon,
	Loader2Icon,
	XCircleIcon,
} from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { itemsFamily } from "../../atoms/messages"
import { streamingVersionFamily } from "../../atoms/streaming"
import { todosFamily } from "../../atoms/todos"
import type { Todo } from "../../lib/types"

/** Native camelCase todo/plan statuses only — no snake_case aliases. */
function normalizeTodoStatus(status: string): string {
	switch (status) {
		case "completed":
		case "inProgress":
		case "cancelled":
		case "pending":
			return status
		default:
			return "pending"
	}
}

function todosFromPlanItem(item: Record<string, unknown>): Todo[] | null {
	const raw = item.entries
	if (!Array.isArray(raw) || raw.length === 0) return null
	const todos = raw
		.map((entry) => {
			if (!entry || typeof entry !== "object") return null
			const value = entry as Record<string, unknown>
			const content = String(value.content ?? value.step ?? "").trim()
			if (!content) return null
			// Skip expanded-looking markdown proposed-plan blobs.
			if (content.includes("\n") && !content.trim().startsWith("{")) return null
			return {
				content,
				status: normalizeTodoStatus(String(value.status ?? "pending")),
			} as Todo
		})
		.filter((todo): todo is Todo => todo !== null)
	return todos.length > 0 ? todos : null
}

/**
 * Derives the latest todo list for a session.
 *
 * Priority order:
 * 1. Store `todos[sessionId]` — set by `todo.updated` Native events (real-time)
 * 2. Fallback: last Native `plan` item's `entries` (session reload)
 */
function useSessionTodos(sessionId: string | null): Todo[] {
	const storeTodos = useAtomValue(todosFamily(sessionId ?? ""))
	const storeItems = useAtomValue(itemsFamily(sessionId ?? ""))
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	return useMemo(() => {
		if (storeTodos && storeTodos.length > 0) return storeTodos

		if (!storeItems || storeItems.length === 0) return []
		void streamingVersion
		for (let i = storeItems.length - 1; i >= 0; i--) {
			const envelope = storeItems[i]
			if (nativeItemType(envelope) !== "plan") continue
			const fromPlan = todosFromPlanItem(envelope.item)
			if (fromPlan) return fromPlan
		}
		return []
	}, [storeTodos, storeItems, streamingVersion, sessionId])
}

/** Compact status icon for a todo item */
function TodoStatusIcon({ status }: { status: string }) {
	switch (normalizeTodoStatus(status)) {
		case "completed":
			return <CheckCircle2Icon className="size-3.5 text-emerald-500/80" />
		case "inProgress":
			return <Loader2Icon className="size-3.5 animate-spin text-blue-400/80" />
		case "cancelled":
			return <XCircleIcon className="size-3.5 text-muted-foreground/30" />
		default:
			return <CircleDotIcon className="size-3.5 text-muted-foreground/30" />
	}
}

interface SessionTaskListProps {
	sessionId: string | null
}

/**
 * Collapsible task list that appears above the input field.
 * Shows the session's current todo list.
 * Subtly styled; task items animate in with stagger and re-animate on status change.
 */
export function SessionTaskList({ sessionId }: SessionTaskListProps) {
	const todos = useSessionTodos(sessionId)
	const [isExpanded, setIsExpanded] = useState(true)
	const scrollRef = useRef<HTMLDivElement>(null)

	const activeTask = useMemo(
		() => todos.find((t) => normalizeTodoStatus(t.status) === "inProgress"),
		[todos],
	)

	const headerLabel = activeTask?.content ?? "Tasks"

	// Auto-scroll to bottom when todos change
	// biome-ignore lint/correctness/useExhaustiveDependencies: scroll on todo changes intentionally
	useEffect(() => {
		if (isExpanded && scrollRef.current) {
			scrollRef.current.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" })
		}
	}, [todos, isExpanded])

	if (todos.length === 0) return null

	return (
		<div className="mb-2 animate-in fade-in overflow-hidden rounded-lg border border-border/60 bg-card shadow-sm duration-400">
			{/* Header — always visible, toggles expansion */}
			<button
				type="button"
				onClick={() => setIsExpanded((prev) => !prev)}
				aria-expanded={isExpanded}
				className={cn(
					"flex w-full items-center gap-2.5 bg-card px-3 py-1.5 text-left transition-colors hover:bg-muted/40",
					isExpanded ? "rounded-t-lg" : "rounded-lg",
				)}
			>
				<span className="min-w-0 flex-1 truncate text-sm text-foreground/80">
					{isExpanded ? "Tasks" : headerLabel}
				</span>

				{/* Chevron indicator */}
				{isExpanded ? (
					<ChevronDownIcon
						className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/60"
						aria-hidden="true"
					/>
				) : (
					<ChevronUpIcon
						className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/60"
						aria-hidden="true"
					/>
				)}
			</button>

			{/* Expandable task list — smooth height transition via grid trick */}
			<div
				className={cn(
					"grid transition-[grid-template-rows] duration-200 ease-out",
					isExpanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
				)}
			>
				<div className="overflow-hidden">
					<div
						ref={scrollRef}
						className="max-h-44 overflow-y-auto border-t border-border/30 px-3 pb-2 pt-1.5"
					>
						<ol className="space-y-1">
							{todos.map((todo, index) => (
								// Key includes status so item re-mounts (fades in fresh) on status change
								// biome-ignore lint/suspicious/noArrayIndexKey: no stable ID in SDK todos
								<li
									key={`${index}-${todo.status}`}
									className="flex items-start gap-2 animate-in fade-in-0 slide-in-from-bottom-1 duration-300"
									style={{ animationDelay: `${index * 35}ms`, animationFillMode: "backwards" }}
								>
									<span className="mt-0.5 shrink-0">
										<TodoStatusIcon status={todo.status} />
									</span>
									<span className="flex items-baseline gap-1.5 text-sm leading-relaxed">
										<span className="shrink-0 tabular-nums text-muted-foreground/40">{index + 1}.</span>
										<span
											className={cn(
												"transition-colors duration-300",
												todo.status === "completed"
													? "text-muted-foreground/50 line-through"
													: todo.status === "cancelled"
														? "text-muted-foreground/40 line-through"
														: todo.status === "inProgress"
															? "text-foreground"
															: "text-muted-foreground",
											)}
										>
											{todo.content}
										</span>
									</span>
								</li>
							))}
						</ol>
					</div>
				</div>
			</div>
		</div>
	)
}
