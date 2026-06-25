/**
 * @mention popover for file and agent references.
 *
 * Shows a searchable list of files and agents when the user types `@`.
 * Matches the design language of the Devo TUI — files show with
 * path + filename, agents show with a brain icon.
 */

import { ScrollArea } from "@devo/ui/components/scroll-area"
import { cn } from "@devo/ui/lib/utils"
import fuzzysort from "fuzzysort"
import { BrainIcon, FileIcon, FolderIcon, SearchIcon } from "lucide-react"
import {
	forwardRef,
	memo,
	useCallback,
	useEffect,
	useImperativeHandle,
	useMemo,
	useRef,
	useState,
} from "react"
import { useFileSearch } from "../../hooks/use-file-search"
import type { SdkAgent } from "../../hooks/use-devo-data"

// ============================================================
// Types
// ============================================================

export type MentionOption =
	| { type: "agent"; name: string; display: string }
	| { type: "file"; path: string; display: string }

export interface MentionPopoverHandle {
	/** Handle keyboard events from the parent textarea. Returns true if consumed. */
	handleKeyDown: (e: React.KeyboardEvent) => boolean
}

interface MentionPopoverProps {
	/** The query text after `@` */
	query: string
	/** Whether the popover is visible */
	open: boolean
	/** Project directory for file search */
	directory: string | null
	/** Available agents */
	agents: SdkAgent[]
	/** Called when a mention is selected */
	onSelect: (option: MentionOption) => void
	/** Called when Escape is pressed */
	onClose: () => void
}

// ============================================================
// Helpers
// ============================================================

function getFileName(path: string): string {
	const parts = path.split("/")
	return parts[parts.length - 1] || path
}

function getDirectory(path: string): string {
	const idx = path.lastIndexOf("/")
	if (idx <= 0) return ""
	return path.slice(0, idx + 1)
}

function isDirectory(path: string): boolean {
	return path.endsWith("/")
}

// ============================================================
// MentionPopover
// ============================================================

export const MentionPopover = memo(
	forwardRef<MentionPopoverHandle, MentionPopoverProps>(function MentionPopover(
		{ query, open, directory, agents, onSelect, onClose },
		ref,
	) {
		const [activeIndex, setActiveIndex] = useState(0)
		const listRef = useRef<HTMLDivElement>(null)

		// --- Data: agents ---
		const agentOptions = useMemo<MentionOption[]>(
			() =>
				agents
					.filter((a) => !a.hidden && a.mode !== "primary")
					.map((a) => ({ type: "agent" as const, name: a.name, display: a.name })),
			[agents],
		)

		// --- Data: file search (enabled whenever popover is open, even with empty query) ---
		const { files } = useFileSearch(directory, query, open)
		const fileOptions = useMemo<MentionOption[]>(
			() => files.slice(0, 20).map((f) => ({ type: "file" as const, path: f, display: f })),
			[files],
		)

		// --- Merge and filter ---
		const allOptions = useMemo<MentionOption[]>(() => {
			if (!query) {
				// No query — show agents + initial files from the server
				return [...agentOptions, ...fileOptions]
			}

			// Fuzzy filter agents
			const agentResults = fuzzysort
				.go(query, agentOptions, { key: "display", threshold: 0.3 })
				.map((r) => r.obj)

			// Files come pre-filtered from the server
			return [...agentResults, ...fileOptions]
		}, [query, agentOptions, fileOptions])

		// Reset active index when options or query change
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset on options/query change
		useEffect(() => {
			setActiveIndex(0)
		}, [allOptions.length, query])

		// Scroll active item into view
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — scroll when active index changes
		useEffect(() => {
			const list = listRef.current
			if (!list) return
			const active = list.querySelector("[data-active=true]")
			if (active) {
				active.scrollIntoView({ block: "nearest" })
			}
		}, [activeIndex])

		// --- Keyboard handler ---
		const handleKeyDown = useCallback(
			(e: React.KeyboardEvent): boolean => {
				if (!open || allOptions.length === 0) return false

				switch (e.key) {
					case "ArrowDown": {
						e.preventDefault()
						setActiveIndex((i) => (i + 1) % allOptions.length)
						return true
					}
					case "ArrowUp": {
						e.preventDefault()
						setActiveIndex((i) => (i - 1 + allOptions.length) % allOptions.length)
						return true
					}
					case "Tab":
					case "Enter": {
						e.preventDefault()
						const selected = allOptions[activeIndex]
						if (selected) onSelect(selected)
						return true
					}
					case "Escape": {
						e.preventDefault()
						onClose()
						return true
					}
					default:
						return false
				}
			},
			[open, allOptions, activeIndex, onSelect, onClose],
		)

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		if (!open) return null

		// --- Group options ---
		const agentItems = allOptions.filter((o) => o.type === "agent")
		const fileItems = allOptions.filter((o) => o.type === "file")
		const hasResults = allOptions.length > 0

		let globalIndex = 0

		return (
			<div
				role="listbox"
				className="absolute inset-x-0 bottom-full z-50 mb-2 origin-bottom-left overflow-hidden rounded-md border bg-popover shadow-md"
				onMouseDown={(e) => e.preventDefault()}
			>
				{/* Search header */}
				<div className="flex items-center gap-2 border-b px-3 py-2">
					<SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
					<span className="text-sm text-muted-foreground">
						{query ? `Searching for "${query}"` : "Mention files or agents"}
					</span>
				</div>

				{/* Results */}
				<ScrollArea className="max-h-64 overflow-hidden [&>[data-slot=scroll-area-viewport]]:max-h-[inherit]">
					<div ref={listRef} className="py-1">
						{!hasResults && (
							<div className="py-4 text-center text-sm text-muted-foreground">
								{query ? "No results found" : "No files or agents available"}
							</div>
						)}

						{/* Agent group */}
						{agentItems.length > 0 && (
							<div>
								<div className="sticky top-0 z-10 border-b bg-popover px-3 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
									Agents
								</div>
								{agentItems.map((option) => {
									const idx = globalIndex++
									return (
										<MentionItem
											key={`agent:${option.type === "agent" ? option.name : ""}`}
											option={option}
											isActive={idx === activeIndex}
											onSelect={() => onSelect(option)}
											onHover={() => setActiveIndex(idx)}
										/>
									)
								})}
							</div>
						)}

						{/* File group */}
						{fileItems.length > 0 && (
							<div>
								<div className="sticky top-0 z-10 border-b bg-popover px-3 py-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
									Files
								</div>
								{fileItems.map((option) => {
									const idx = globalIndex++
									const path = option.type === "file" ? option.path : ""
									return (
										<MentionItem
											key={`file:${path}`}
											option={option}
											isActive={idx === activeIndex}
											onSelect={() => onSelect(option)}
											onHover={() => setActiveIndex(idx)}
										/>
									)
								})}
							</div>
						)}
					</div>
				</ScrollArea>
			</div>
		)
	}),
)

// ============================================================
// MentionItem
// ============================================================

const MentionItem = memo(function MentionItem({
	option,
	isActive,
	onSelect,
	onHover,
}: {
	option: MentionOption
	isActive: boolean
	onSelect: () => void
	onHover: () => void
}) {
	if (option.type === "agent") {
		return (
			<button
				type="button"
				data-active={isActive}
				className={cn(
					"flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors",
					isActive ? "bg-accent text-accent-foreground" : "hover:bg-muted",
				)}
				onClick={onSelect}
				onMouseEnter={onHover}
			>
				<BrainIcon className="size-4 shrink-0 text-blue-400" />
				<span className="font-medium">@{option.name}</span>
			</button>
		)
	}

	const path = option.path
	const dir = getDirectory(path)
	const name = getFileName(path)
	const isDir = isDirectory(path)

	return (
		<button
			type="button"
			data-active={isActive}
			className={cn(
				"flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors",
				isActive ? "bg-accent text-accent-foreground" : "hover:bg-muted",
			)}
			onClick={onSelect}
			onMouseEnter={onHover}
		>
			{isDir ? (
				<FolderIcon className="size-4 shrink-0 text-muted-foreground" />
			) : (
				<FileIcon className="size-4 shrink-0 text-muted-foreground" />
			)}
			<div className="flex min-w-0 items-center">
				<span className="font-medium">{name}</span>
				{dir && <span className="ml-1.5 truncate text-muted-foreground">{dir}</span>}
			</div>
		</button>
	)
})
