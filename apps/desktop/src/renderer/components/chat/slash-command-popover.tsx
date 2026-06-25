/**
 * Slash command popover — appears when the user types `/` in the input.
 *
 * Matches the Devo TUI pattern:
 * - Flat command list with fuzzy search
 * - Skills are excluded (accessible via /skills → opens a dedicated picker)
 * - MCP commands show a `:mcp` badge
 * - Keyboard navigation (Arrow keys, Enter/Tab, Escape)
 */

import { ScrollArea } from "@devo/ui/components/scroll-area"
import { cn } from "@devo/ui/lib/utils"
import fuzzysort from "fuzzysort"
import {
	BookOpenIcon,
	CodeIcon,
	GitForkIcon,
	type LucideIcon,
	MessageSquareIcon,
	Redo2Icon,
	SearchIcon,
	SettingsIcon,
	SparklesIcon,
	TerminalIcon,
	Undo2Icon,
	WrenchIcon,
} from "lucide-react"
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
import { useServerCommands } from "../../hooks/use-devo-data"
import { formatShortcut } from "../../lib/shortcut-display"

// ============================================================
// Types
// ============================================================

interface SlashCommand {
	name: string
	description: string
	icon: LucideIcon
	source: "client" | "server"
	/** For server commands: the original source type */
	serverSource?: "command" | "mcp" | "skill"
	/** Keyboard shortcut */
	shortcut?: string
	/** Special action instead of regular command execution */
	action?: "skills" | "fork"
}

export interface SlashCommandPopoverHandle {
	/** Handle keyboard events from the parent textarea. Returns true if consumed. */
	handleKeyDown: (e: React.KeyboardEvent) => boolean
}

interface SlashCommandPopoverProps {
	/** The query text after `/` */
	query: string
	/** Whether the popover is visible */
	open: boolean
	/** Whether the popover should be active (connected, has session, etc.) */
	enabled: boolean
	/** Directory for fetching server commands */
	directory: string | null
	/** Callback when a command is selected */
	onSelect: (command: string) => void
	/** Called when the /skills entry is selected — opens the skills picker */
	onSkillsOpen?: () => void
	/** Called when the /fork entry is selected — forks the session */
	onFork?: () => Promise<void>
	/** Called when Escape is pressed */
	onClose: () => void
}

// ============================================================
// Built-in client commands
// ============================================================

const CLIENT_COMMANDS: SlashCommand[] = [
	{
		name: "undo",
		description: "Undo the last turn",
		icon: Undo2Icon,
		source: "client",
		shortcut: formatShortcut(["mod", "Z"]),
	},
	{
		name: "redo",
		description: "Redo previously undone turn",
		icon: Redo2Icon,
		source: "client",
		shortcut: formatShortcut(["shift", "mod", "Z"]),
	},
	{
		name: "compact",
		description: "Summarize conversation to save context",
		icon: SparklesIcon,
		source: "client",
	},
	{
		name: "fork",
		description: "Fork conversation from this point",
		icon: GitForkIcon,
		source: "client",
		action: "fork",
	},
	{
		name: "skills",
		description: "Browse and use skills",
		icon: BookOpenIcon,
		source: "client",
		action: "skills",
	},
]

function getCommandIcon(name: string): LucideIcon {
	switch (name) {
		case "init":
			return SettingsIcon
		case "review":
			return CodeIcon
		case "feedback":
			return MessageSquareIcon
		case "mcp":
			return WrenchIcon
		default:
			return TerminalIcon
	}
}

// ============================================================
// SlashCommandPopover
// ============================================================

export const SlashCommandPopover = memo(
	forwardRef<SlashCommandPopoverHandle, SlashCommandPopoverProps>(function SlashCommandPopover(
		{ query, open, directory, onSelect, onSkillsOpen, onFork, onClose },
		ref,
	) {
		const [activeIndex, setActiveIndex] = useState(0)
		const listRef = useRef<HTMLDivElement>(null)

		// --- Server commands (skills excluded, matching TUI pattern) ---
		const rawServerCommands = useServerCommands(directory)
		const serverCommands = useMemo<SlashCommand[]>(
			() =>
				rawServerCommands
					.filter((c) => c.source !== "skill")
					.map((c) => ({
						name: c.name,
						description: c.description ?? `Run /${c.name}`,
						icon: getCommandIcon(c.name),
						source: "server" as const,
						serverSource: c.source,
					})),
			[rawServerCommands],
		)

		// --- Merge: server commands first, then built-in (matching TUI ordering) ---
		const allCommands = useMemo(() => [...serverCommands, ...CLIENT_COMMANDS], [serverCommands])

		// --- Fuzzy filter ---
		const flatList = useMemo<SlashCommand[]>(() => {
			if (!query) return allCommands
			const results = fuzzysort.go(query, allCommands, {
				keys: ["name", "description"],
				threshold: 0.3,
			})
			return results.map((r) => r.obj)
		}, [allCommands, query])

		// Reset active index when options or query change
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset on options/query change
		useEffect(() => {
			setActiveIndex(0)
		}, [flatList.length, query])

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

		// --- Handle selection (regular commands or special actions) ---
		const handleSelect = useCallback(
			(cmd: SlashCommand) => {
				if (cmd.action === "skills") {
					onClose()
					onSkillsOpen?.()
				} else if (cmd.action === "fork") {
					onClose()
					onFork?.()
				} else {
					onSelect(`/${cmd.name}`)
				}
			},
			[onSelect, onClose, onSkillsOpen, onFork],
		)

		// --- Keyboard handler ---
		const handleKeyDown = useCallback(
			(e: React.KeyboardEvent): boolean => {
				if (!open || flatList.length === 0) return false

				switch (e.key) {
					case "ArrowDown": {
						e.preventDefault()
						setActiveIndex((i) => (i + 1) % flatList.length)
						return true
					}
					case "ArrowUp": {
						e.preventDefault()
						setActiveIndex((i) => (i - 1 + flatList.length) % flatList.length)
						return true
					}
					case "Tab":
					case "Enter": {
						e.preventDefault()
						const selected = flatList[activeIndex]
						if (selected) handleSelect(selected)
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
			[open, flatList, activeIndex, handleSelect, onClose],
		)

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		if (!open) return null

		return (
			<div
				role="listbox"
				className="absolute inset-x-0 bottom-full z-50 mb-2 origin-bottom-left overflow-hidden rounded-md border bg-popover shadow-md"
				onMouseDown={(e) => e.preventDefault()}
			>
				{/* Search header */}
				<div className="flex items-center gap-2 border-b px-3 py-2">
					<SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
					<span className="text-sm text-muted-foreground">Search</span>
				</div>

				{/* Results */}
				<ScrollArea className="max-h-72 overflow-hidden [&>[data-slot=scroll-area-viewport]]:max-h-[inherit]">
					<div ref={listRef} className="py-1">
						{flatList.length === 0 && (
							<div className="py-4 text-center text-sm text-muted-foreground">
								No commands found
							</div>
						)}

						{flatList.map((cmd, idx) => (
							<CommandItem
								key={cmd.name}
								command={cmd}
								isActive={idx === activeIndex}
								onSelect={() => handleSelect(cmd)}
								onHover={() => setActiveIndex(idx)}
							/>
						))}
					</div>
				</ScrollArea>
			</div>
		)
	}),
)

// ============================================================
// CommandItem
// ============================================================

const CommandItem = memo(function CommandItem({
	command,
	isActive,
	onSelect,
	onHover,
}: {
	command: SlashCommand
	isActive: boolean
	onSelect: () => void
	onHover: () => void
}) {
	const Icon = command.icon

	return (
		<button
			type="button"
			data-active={isActive}
			className={cn(
				"flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left text-sm transition-colors",
				isActive ? "bg-accent text-accent-foreground" : "hover:bg-muted",
			)}
			onClick={onSelect}
			onMouseEnter={onHover}
		>
			<div className="flex min-w-0 items-center gap-2">
				<Icon className="size-4 shrink-0 text-muted-foreground" />
				<span className="font-medium">/{command.name}</span>
				{command.description && (
					<span className="truncate text-muted-foreground">{command.description}</span>
				)}
			</div>
			<div className="flex shrink-0 items-center gap-2">
				{command.serverSource === "mcp" && (
					<span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
						mcp
					</span>
				)}
				{command.shortcut && (
					<span className="text-xs text-muted-foreground">{command.shortcut}</span>
				)}
			</div>
		</button>
	)
})
