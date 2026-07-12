/**
 * Context items display — reference/agent mention chips shown above the input.
 *
 * Inspired by Devo TUI's context-items.tsx pattern.
 * Shows removable chips for each tracked @ mention.
 */
import { cn } from "@devo/ui/lib/utils"
import { BrainIcon, FileIcon, PlugIcon, SparklesIcon, XIcon } from "lucide-react"
import { memo } from "react"
import { getMentionKey, getMentionMarker, type PromptMention } from "./prompt-mentions"

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
					key={getMentionKey(mention)}
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
	const isFile = mention.type === "file"
	const label = isAgent
		? `@${mention.name}`
		: isFile
			? getFileName(mention.path)
			: getMentionMarker(mention)
	const tooltip = isAgent
		? `Agent: ${mention.name}`
		: isFile
			? mention.path
			: `${mention.kind === "skill" ? "Skill" : "MCP"}: ${mention.name}`
	const referenceClass =
		mention.type === "reference"
			? mention.kind === "skill"
				? "bg-cyan-500/10 text-cyan-600 dark:text-cyan-400"
				: "bg-fuchsia-500/10 text-fuchsia-600 dark:text-fuchsia-400"
			: undefined

	return (
		<span
			title={tooltip}
			className={cn(
				"group inline-flex max-w-[200px] items-center gap-1 rounded-md px-2 py-0.5 text-xs font-medium transition-colors",
				isAgent
					? "bg-blue-500/10 text-blue-400"
					: referenceClass ?? "bg-muted text-muted-foreground",
			)}
		>
			{isAgent ? (
				<BrainIcon className="size-3 shrink-0 stroke-[1.5]" />
			) : isFile ? (
				<FileIcon className="size-3 shrink-0 stroke-[1.5]" />
			) : mention.kind === "skill" ? (
				<SparklesIcon className="size-3 shrink-0 stroke-[1.5]" />
			) : (
				<PlugIcon className="size-3 shrink-0 stroke-[1.5]" />
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
				<XIcon className="size-3 stroke-[1.5]" />
			</button>
		</span>
	)
})
