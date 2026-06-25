/**
 * Review panel inline commenting system.
 *
 * Provides:
 * - DiffComment type and atom for per-session comment storage
 * - DiffCommentButton: rendered via @pierre/diffs renderHoverUtility
 * - ReviewPanelComments: pills displayed in the panel
 * - useDiffComments hook: manages comment CRUD
 */
import { atom, useAtomValue, useSetAtom } from "jotai"
import { atomFamily } from "jotai/utils"
import { MessageSquarePlusIcon, XIcon } from "lucide-react"
import { useCallback, useRef, useState } from "react"

// ============================================================
// Types
// ============================================================

export interface DiffComment {
	id: string
	filePath: string
	lineNumber: number
	side: "additions" | "deletions"
	content: string
	createdAt: number
}

// ============================================================
// Atoms
// ============================================================

/** Per-session comment store */
export const diffCommentsFamily = atomFamily((_sessionId: string) => atom<DiffComment[]>([]))

// ============================================================
// Hook
// ============================================================

export function useDiffComments(sessionId: string) {
	const comments = useAtomValue(diffCommentsFamily(sessionId))
	const setComments = useSetAtom(diffCommentsFamily(sessionId))

	const addComment = useCallback(
		(input: {
			filePath: string
			lineNumber: number
			side: "additions" | "deletions"
			content: string
		}) => {
			const comment: DiffComment = {
				id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
				...input,
				createdAt: Date.now(),
			}
			setComments((prev) => [...prev, comment])
		},
		[setComments],
	)

	const removeComment = useCallback(
		(commentId: string) => {
			setComments((prev) => prev.filter((c) => c.id !== commentId))
		},
		[setComments],
	)

	const clearComments = useCallback(() => {
		setComments([])
	}, [setComments])

	return { comments, addComment, removeComment, clearComments }
}

// ============================================================
// DiffCommentButton -- rendered via renderHoverUtility
// ============================================================

interface DiffCommentButtonProps {
	filePath: string
	getHoveredLine: () => { lineNumber: number; side: "additions" | "deletions" } | undefined
	onAddComment: (comment: {
		filePath: string
		lineNumber: number
		side: "additions" | "deletions"
		content: string
	}) => void
}

export function DiffCommentButton({
	filePath,
	getHoveredLine,
	onAddComment,
}: DiffCommentButtonProps) {
	const [showInput, setShowInput] = useState(false)
	const [value, setValue] = useState("")
	const lineInfoRef = useRef<{ lineNumber: number; side: "additions" | "deletions" } | null>(null)
	const inputRef = useRef<HTMLTextAreaElement>(null)

	const handleClick = useCallback(() => {
		const info = getHoveredLine()
		if (!info) return
		lineInfoRef.current = {
			lineNumber: info.lineNumber,
			side: info.side,
		}
		setShowInput(true)
		setValue("")
		requestAnimationFrame(() => inputRef.current?.focus())
	}, [getHoveredLine])

	const handleSubmit = useCallback(() => {
		const trimmed = value.trim()
		if (!trimmed || !lineInfoRef.current) return
		onAddComment({
			filePath,
			lineNumber: lineInfoRef.current.lineNumber,
			side: lineInfoRef.current.side,
			content: trimmed,
		})
		setShowInput(false)
		setValue("")
	}, [value, filePath, onAddComment])

	const handleCancel = useCallback(() => {
		setShowInput(false)
		setValue("")
	}, [])

	const handleKeyDown = useCallback(
		(e: React.KeyboardEvent) => {
			if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
				e.preventDefault()
				handleSubmit()
			}
			if (e.key === "Escape") {
				e.preventDefault()
				handleCancel()
			}
		},
		[handleSubmit, handleCancel],
	)

	if (showInput) {
		return (
			<div
				className="absolute left-0 right-0 z-20 border-y border-border bg-background p-2"
				style={{ top: "100%" }}
			>
				<textarea
					ref={inputRef}
					value={value}
					onChange={(e) => setValue(e.target.value)}
					onKeyDown={handleKeyDown}
					placeholder="Request change..."
					className="w-full resize-none rounded-md border border-border bg-muted/50 px-2.5 py-1.5 font-sans text-xs text-foreground placeholder:text-muted-foreground/50 focus:border-primary focus:outline-none"
					rows={2}
				/>
				<div className="mt-1.5 flex items-center justify-end gap-1.5">
					<button
						type="button"
						onClick={handleCancel}
						className="rounded-md px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
					>
						Cancel
					</button>
					<button
						type="button"
						onClick={handleSubmit}
						disabled={!value.trim()}
						className="rounded-md bg-primary px-2 py-1 text-[11px] font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
					>
						Comment
					</button>
				</div>
			</div>
		)
	}

	return (
		<button
			type="button"
			onClick={handleClick}
			className="flex size-4 items-center justify-center rounded bg-primary text-primary-foreground opacity-0 transition-opacity hover:bg-primary/90 group-hover:opacity-100"
			title="Add comment"
		>
			<MessageSquarePlusIcon className="size-2.5" />
		</button>
	)
}

// ============================================================
// ReviewPanelComments -- displayed in the panel header area
// ============================================================

interface ReviewPanelCommentsProps {
	comments: DiffComment[]
	onRemove: (commentId: string) => void
	onClear: () => void
}

export function ReviewPanelComments({ comments, onRemove, onClear }: ReviewPanelCommentsProps) {
	return (
		<div className="shrink-0 border-b border-border px-3 py-2">
			<div className="flex items-center justify-between">
				<span className="text-[11px] font-medium text-muted-foreground">
					{comments.length} comment{comments.length !== 1 ? "s" : ""} attached
				</span>
				<button
					type="button"
					onClick={onClear}
					className="text-[10px] text-muted-foreground/60 transition-colors hover:text-foreground"
				>
					Clear all
				</button>
			</div>
			<div className="mt-1.5 flex flex-wrap gap-1">
				{comments.map((comment) => (
					<CommentPill key={comment.id} comment={comment} onRemove={onRemove} />
				))}
			</div>
		</div>
	)
}

function CommentPill({
	comment,
	onRemove,
}: {
	comment: DiffComment
	onRemove: (id: string) => void
}) {
	const fileName = comment.filePath.split("/").pop() ?? comment.filePath

	return (
		<span className="inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-muted/50 px-1.5 py-0.5 text-[10px] leading-tight">
			<span className="truncate text-muted-foreground" title={comment.filePath}>
				{fileName}:{comment.lineNumber}
			</span>
			<span className="truncate text-foreground" title={comment.content}>
				{comment.content.length > 30 ? `${comment.content.slice(0, 30)}...` : comment.content}
			</span>
			<button
				type="button"
				onClick={() => onRemove(comment.id)}
				className="shrink-0 text-muted-foreground/60 hover:text-foreground"
			>
				<XIcon className="size-2.5" />
			</button>
		</span>
	)
}

// ============================================================
// Serialize comments to structured text for chat messages
// ============================================================

/**
 * Serialize diff comments into structured context to prepend to a user message.
 * Returns empty string if no comments exist.
 */
export function serializeCommentsForChat(comments: DiffComment[]): string {
	if (comments.length === 0) return ""

	const lines = ["[Code Review Comments]", ""]
	for (const c of comments) {
		const side = c.side === "additions" ? "new" : "old"
		lines.push(`- ${c.filePath}:${c.lineNumber} (${side}): ${c.content}`)
	}
	lines.push("")
	return lines.join("\n")
}
