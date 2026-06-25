/**
 * Context items display — file/agent mention chips shown above the input.
 *
 * Inspired by Devo TUI's context-items.tsx pattern.
 * Shows removable chips for each @-mentioned file or agent.
 */
import { cn } from "@devo/ui/lib/utils"
import { BrainIcon, FileIcon, XIcon } from "lucide-react"
import { memo } from "react"
import type { PromptMention } from "./prompt-mentions"

// ============================================================
// ContextItems
// ============================================================

interface ContextItemsProps {
	mentions: PromptMention[]
	onRemove: (mention: PromptMention) => void
	className?: string
}

export const ContextItems = memo(function ContextItems({
	mentions,
	onRemove,
	className,
}: ContextItemsProps) {
	if (mentions.length === 0) return null

	return (
		<div
			className={cn(
				"flex w-full flex-wrap items-center justify-start gap-1.5 px-3 pt-2",
				className,
			)}
		>
			{mentions.map((mention) => (
				<ContextChip
					key={mention.type === "file" ? `file:${mention.path}` : `agent:${mention.name}`}
					mention={mention}
					onRemove={() => onRemove(mention)}
				/>
			))}
		</div>
	)
})

// ============================================================
// ContextChip
// ============================================================

function getFileName(path: string): string {
	const parts = path.split("/")
	return parts[parts.length - 1] || path
}

const ContextChip = memo(function ContextChip({
	mention,
	onRemove,
}: {
	mention: PromptMention
	onRemove: () => void
}) {
	const isAgent = mention.type === "agent"
	const label = isAgent ? `@${mention.name}` : getFileName(mention.path)
	const tooltip = isAgent ? `Agent: ${mention.name}` : mention.path

	return (
		<span
			title={tooltip}
			className={cn(
				"group inline-flex max-w-[200px] items-center gap-1 rounded-md px-2 py-0.5 text-xs font-medium transition-colors",
				isAgent ? "bg-blue-500/10 text-blue-400" : "bg-muted text-muted-foreground",
			)}
		>
			{isAgent ? (
				<BrainIcon className="size-3 shrink-0" />
			) : (
				<FileIcon className="size-3 shrink-0" />
			)}
			<span className="truncate">{label}</span>
			<button
				type="button"
				onClick={(e) => {
					e.stopPropagation()
					onRemove()
				}}
				className="ml-0.5 shrink-0 rounded-sm opacity-50 transition-opacity hover:opacity-100"
				aria-label={`Remove ${label}`}
			>
				<XIcon className="size-3" />
			</button>
		</span>
	)
})
