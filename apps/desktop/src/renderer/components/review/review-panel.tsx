/**
 * ReviewPanel -- right-side collapsible panel for viewing session file diffs.
 *
 * Performance strategy (layered):
 * 1. Generated files (lockfiles, build output) start collapsed -- no render cost
 * 2. When >AUTO_COLLAPSE_THRESHOLD files, ALL diffs start collapsed (header only)
 * 3. Large diffs (>LARGE_DIFF_LINE_THRESHOLD lines) show a "Load diff" gate
 * 4. TanStack Virtual virtualizes the diff list -- only visible items are in the DOM
 * 5. @pierre/diffs WorkerPoolContext offloads Shiki highlighting to web workers
 * 6. Stable memoized objects prevent @pierre/diffs re-parsing unchanged content
 * 7. Only the active theme (light/dark) is rendered, not both
 */
import { cn } from "@devo/ui/lib/utils"
import { MultiFileDiff, useWorkerPool, WorkerPoolContextProvider } from "@pierre/diffs/react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import {
	AlertTriangleIcon,
	ChevronDownIcon,
	ChevronRightIcon,
	ChevronsDownUpIcon,
	ChevronsUpDownIcon,
	ColumnsIcon,
	FileIcon,
	Loader2Icon,
	MaximizeIcon,
	MinimizeIcon,
	MinusIcon,
	PlusIcon,
	RowsIcon,
	XIcon,
} from "lucide-react"
import {
	memo,
	type ReactNode,
	startTransition,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react"
import {
	type DiffStyle,
	reviewPanelOpenAtom,
	reviewPanelSelectedFileAtom,
	reviewPanelSettingsAtom,
} from "../../atoms/ui"
import { useSessionDiff } from "../../hooks/use-session-diff"
import type { FileDiff } from "../../lib/types"
import { DiffCommentButton, ReviewPanelComments, useDiffComments } from "./review-comments"

// ============================================================
// Constants
// ============================================================

/**
 * When the total file count exceeds this threshold, all diffs start
 * collapsed (header-only) to avoid mounting dozens of expensive
 * syntax-highlighted views on initial render.
 */
const AUTO_COLLAPSE_THRESHOLD = 12

/** Max total lines changed before a diff shows a "Load diff" gate. */
const LARGE_DIFF_LINE_THRESHOLD = 1500

/** Estimated height (px) of a collapsed diff section (header only). */
const COLLAPSED_ROW_HEIGHT = 32

// ============================================================
// Generated / vendor file detection
// ============================================================

/**
 * Patterns for files considered "generated" -- lockfiles, build output, vendored
 * deps, etc. These files are always shown in the panel, but their diff sections
 * start collapsed so they don't slow down initial render.
 */
const GENERATED_FILE_PATTERNS: RegExp[] = [
	/(?:^|\/)bun\.lock$/,
	/(?:^|\/)bun\.lockb$/,
	/(?:^|\/)package-lock\.json$/,
	/(?:^|\/)yarn\.lock$/,
	/(?:^|\/)pnpm-lock\.yaml$/,
	/(?:^|\/)Gemfile\.lock$/,
	/(?:^|\/)Cargo\.lock$/,
	/(?:^|\/)composer\.lock$/,
	/(?:^|\/)poetry\.lock$/,
	/(?:^|\/)Pipfile\.lock$/,
	/(?:^|\/)go\.sum$/,
	/(?:^|\/)flake\.lock$/,
	/(?:^|\/)dist\//,
	/(?:^|\/)build\//,
	/(?:^|\/)\.next\//,
	/(?:^|\/)out\//,
	/(?:^|\/)vendor\//,
	/(?:^|\/)node_modules\//,
	/\.map$/,
	/\.min\.(js|css)$/,
	/(?:^|\/)\.generated\./,
	/\.g\.(ts|js)$/,
	/\.gen\.(ts|js)$/,
]

function isGeneratedFile(filePath: string): boolean {
	return GENERATED_FILE_PATTERNS.some((p) => p.test(filePath))
}

function isLargeDiff(diff: FileDiff): boolean {
	return diff.additions + diff.deletions > LARGE_DIFF_LINE_THRESHOLD
}

// ============================================================
// Worker pool factory (Vite-compatible)
// ============================================================

/**
 * Creates a new Web Worker for the @pierre/diffs worker pool.
 * Uses Vite's `?worker` import pattern for correct bundling.
 */
function workerFactory(): Worker {
	return new Worker(new URL("@pierre/diffs/worker/worker.js", import.meta.url), {
		type: "module",
	})
}

/** Stable pool options object (never changes, avoids provider re-renders). */
const WORKER_POOL_OPTIONS = {
	workerFactory,
	poolSize: 4,
} as const

// ============================================================
// Theme detection (render only one theme, not both)
// ============================================================

function useIsDarkMode(): boolean {
	const [dark, setDark] = useState(
		() =>
			document.documentElement.classList.contains("dark") ||
			document.documentElement.dataset.theme === "dark",
	)
	useEffect(() => {
		const observer = new MutationObserver(() => {
			setDark(
				document.documentElement.classList.contains("dark") ||
					document.documentElement.dataset.theme === "dark",
			)
		})
		observer.observe(document.documentElement, {
			attributes: true,
			attributeFilter: ["class", "data-theme"],
		})
		return () => observer.disconnect()
	}, [])
	return dark
}

// ============================================================
// Main ReviewPanel component
// ============================================================

interface ReviewPanelProps {
	sessionId: string
	directory: string
	className?: string
}

export const ReviewPanel = memo(function ReviewPanel({
	sessionId,
	directory,
	className,
}: ReviewPanelProps) {
	const { diffs, stats, loading } = useSessionDiff(sessionId, directory)
	const [settings, setSettings] = useAtom(reviewPanelSettingsAtom)
	const setOpen = useAtom(reviewPanelOpenAtom)[1]
	const [selectedFile, setSelectedFile] = useState<string | null>(null)
	const { comments, addComment, removeComment, clearComments } = useDiffComments(sessionId)

	// --- External file selection (e.g. "View diff" button in tool cards) ---
	const externalFile = useAtomValue(reviewPanelSelectedFileAtom)
	const clearExternalFile = useSetAtom(reviewPanelSelectedFileAtom)
	useEffect(() => {
		if (!externalFile || diffs.length === 0) return
		// The tool card sends an absolute path; diff entries use relative paths.
		// Match by suffix: find the diff whose relative path is a suffix of the
		// absolute path (or vice versa).
		const match = diffs.find(
			(d) =>
				d.file === externalFile ||
				externalFile.endsWith(`/${d.file}`) ||
				d.file.endsWith(`/${externalFile}`),
		)
		if (match) {
			setSelectedFile(match.file)
			setUserToggles((prev) => ({ ...prev, [match.file]: true }))
		}
		clearExternalFile(null)
	}, [externalFile, clearExternalFile, diffs])

	// --- Collapse state management ---
	// Track which files the user has explicitly toggled (overrides auto-collapse).
	// Key = file path, value = true (expanded) | false (collapsed).
	const [userToggles, setUserToggles] = useState<Record<string, boolean>>({})

	const manyFiles = diffs.length > AUTO_COLLAPSE_THRESHOLD

	const getIsCollapsed = useCallback(
		(diff: FileDiff): boolean => {
			// User override takes priority
			if (diff.file in userToggles) return !userToggles[diff.file]
			// Auto-collapse rules
			if (manyFiles) return true
			if (isGeneratedFile(diff.file)) return true
			return false
		},
		[userToggles, manyFiles],
	)

	const toggleFile = useCallback(
		(file: string) => {
			setUserToggles((prev) => {
				// Compute current expanded state from prev toggles (no external deps)
				const wasExpanded =
					file in prev
						? prev[file]
						: // Default: expanded unless auto-collapse rules apply
							!(manyFiles || isGeneratedFile(file))
				return { ...prev, [file]: !wasExpanded }
			})
		},
		[manyFiles],
	)

	const collapseAll = useCallback(() => {
		const next: Record<string, boolean> = {}
		for (const d of diffs) next[d.file] = false
		setUserToggles(next)
	}, [diffs])

	const expandAll = useCallback(() => {
		const next: Record<string, boolean> = {}
		for (const d of diffs) next[d.file] = true
		setUserToggles(next)
	}, [diffs])

	// Reset user toggles when session changes
	const prevSessionRef = useRef(sessionId)
	useEffect(() => {
		if (prevSessionRef.current !== sessionId) {
			prevSessionRef.current = sessionId
			setUserToggles({})
		}
	}, [sessionId])

	// --- Handlers ---
	const handleClose = useCallback(() => setOpen(false), [setOpen])
	const handleToggleExpanded = useCallback(
		() => setSettings((prev) => ({ ...prev, expanded: !prev.expanded })),
		[setSettings],
	)
	const handleSetDiffStyle = useCallback(
		(style: DiffStyle) => setSettings((prev) => ({ ...prev, diffStyle: style })),
		[setSettings],
	)

	// Apply file selection filter
	const displayedDiffs = useMemo(() => {
		if (!selectedFile) return diffs
		return diffs.filter((d) => d.file === selectedFile)
	}, [diffs, selectedFile])

	// Count how many are currently expanded (for the toggle icon)
	const expandedCount = useMemo(() => {
		return displayedDiffs.filter((d) => !getIsCollapsed(d)).length
	}, [displayedDiffs, getIsCollapsed])

	return (
		<div className={cn("flex h-full flex-col overflow-hidden bg-background", className)}>
			{/* Panel header */}
			<div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-2">
				<div className="flex items-center gap-2">
					<h3 className="text-xs font-semibold text-foreground">Changes</h3>
					{stats.fileCount > 0 && (
						<span className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
							<span className="flex items-center gap-0.5 text-green-500">
								<PlusIcon className="size-2.5" aria-hidden="true" />
								{stats.additions}
							</span>
							<span className="flex items-center gap-0.5 text-red-500">
								<MinusIcon className="size-2.5" aria-hidden="true" />
								{stats.deletions}
							</span>
						</span>
					)}
				</div>
				<div className="flex items-center gap-0.5">
					{/* Collapse / Expand all */}
					{displayedDiffs.length > 1 && (
						<button
							type="button"
							onClick={expandedCount > 0 ? collapseAll : expandAll}
							className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
							title={expandedCount > 0 ? "Collapse all" : "Expand all"}
						>
							{expandedCount > 0 ? (
								<ChevronsDownUpIcon className="size-3.5" />
							) : (
								<ChevronsUpDownIcon className="size-3.5" />
							)}
						</button>
					)}
					{/* Diff style toggle */}
					<button
						type="button"
						onClick={() =>
							handleSetDiffStyle(settings.diffStyle === "unified" ? "split" : "unified")
						}
						className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						title={
							settings.diffStyle === "unified" ? "Switch to split view" : "Switch to unified view"
						}
					>
						{settings.diffStyle === "unified" ? (
							<ColumnsIcon className="size-3.5" />
						) : (
							<RowsIcon className="size-3.5" />
						)}
					</button>
					{/* Expand / collapse panel */}
					<button
						type="button"
						onClick={handleToggleExpanded}
						className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						title={settings.expanded ? "Restore panel size" : "Expand to full width"}
					>
						{settings.expanded ? (
							<MinimizeIcon className="size-3.5" />
						) : (
							<MaximizeIcon className="size-3.5" />
						)}
					</button>
					{/* Close */}
					<button
						type="button"
						onClick={handleClose}
						className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
					>
						<XIcon className="size-3.5" />
					</button>
				</div>
			</div>

			{/* File list */}
			{diffs.length > 0 && (
				<div className="shrink-0 border-b border-border">
					<FileList diffs={diffs} selectedFile={selectedFile} onSelectFile={setSelectedFile} />
				</div>
			)}

			{/* Comment pills */}
			{comments.length > 0 && (
				<ReviewPanelComments comments={comments} onRemove={removeComment} onClear={clearComments} />
			)}

			{/* Diff content -- virtualized */}
			<div className="min-h-0 flex-1">
				{loading ? (
					<div className="flex items-center justify-center py-12">
						<Loader2Icon className="size-5 animate-spin text-muted-foreground" />
						<span className="ml-2 text-sm text-muted-foreground">Loading changes...</span>
					</div>
				) : diffs.length === 0 ? (
					<EmptyState />
				) : (
					<VirtualizedDiffList
						diffs={displayedDiffs}
						diffStyle={settings.diffStyle}
						getIsCollapsed={getIsCollapsed}
						onToggle={toggleFile}
						onAddComment={addComment}
					/>
				)}
			</div>
		</div>
	)
})

// ============================================================
// Virtualized diff list using TanStack Virtual
// ============================================================

interface VirtualizedDiffListProps {
	diffs: FileDiff[]
	diffStyle: DiffStyle
	getIsCollapsed: (diff: FileDiff) => boolean
	onToggle: (file: string) => void
	onAddComment: (comment: {
		filePath: string
		lineNumber: number
		side: "additions" | "deletions"
		content: string
	}) => void
}

const VirtualizedDiffList = memo(function VirtualizedDiffList({
	diffs,
	diffStyle,
	getIsCollapsed,
	onToggle,
	onAddComment,
}: VirtualizedDiffListProps) {
	const scrollRef = useRef<HTMLDivElement>(null)
	const isDark = useIsDarkMode()
	const [pinnedDiff, setPinnedDiff] = useState<FileDiff | null>(null)
	const pinnedFileRef = useRef<string | null>(null)

	const theme = isDark ? ("one-dark-pro" as const) : ("one-light" as const)

	// Stable highlighter options for the worker pool (theme + lineDiffType).
	// When using the worker pool, these are controlled by the pool, not per-component.
	const highlighterOptions = useMemo(
		() => ({
			theme,
			lineDiffType: "word" as const,
		}),
		[theme],
	)

	const virtualizer = useVirtualizer({
		count: diffs.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: (index) => {
			const diff = diffs[index]
			if (getIsCollapsed(diff)) return COLLAPSED_ROW_HEIGHT
			if (isLargeDiff(diff)) return COLLAPSED_ROW_HEIGHT + 80 // header + placeholder
			// Rough estimate based on line count (collapsed unchanged hunks help)
			const lines = Math.min(diff.additions + diff.deletions, 200)
			return COLLAPSED_ROW_HEIGHT + lines * 20
		},
		overscan: 3,
	})

	// --- Pinned header: detect which expanded file's header has scrolled past ---
	useEffect(() => {
		const el = scrollRef.current
		if (!el) return
		let rafId: number | null = null

		const onScroll = () => {
			if (rafId !== null) return
			rafId = requestAnimationFrame(() => {
				rafId = null
				const scrollTop = el.scrollTop
				// Near the top -- nothing to pin
				if (scrollTop < COLLAPSED_ROW_HEIGHT) {
					if (pinnedFileRef.current !== null) {
						pinnedFileRef.current = null
						setPinnedDiff(null)
					}
					return
				}
				// Find the expanded file whose header has fully scrolled out of view
				// but whose body still extends below the viewport top
				let found: FileDiff | null = null
				for (const item of virtualizer.getVirtualItems()) {
					const diff = diffs[item.index]
					if (getIsCollapsed(diff)) continue
					const headerBottom = item.start + COLLAPSED_ROW_HEIGHT
					if (
						headerBottom <= scrollTop &&
						item.start + item.size > scrollTop + COLLAPSED_ROW_HEIGHT
					) {
						found = diff
						break
					}
				}
				const foundFile = found?.file ?? null
				if (pinnedFileRef.current !== foundFile) {
					pinnedFileRef.current = foundFile
					setPinnedDiff(found)
				}
			})
		}

		el.addEventListener("scroll", onScroll, { passive: true })
		return () => {
			el.removeEventListener("scroll", onScroll)
			if (rafId !== null) cancelAnimationFrame(rafId)
		}
	}, [diffs, getIsCollapsed]) // virtualizer accessed via closure, always current

	const handlePinnedToggle = useCallback(() => {
		if (pinnedDiff) onToggle(pinnedDiff.file)
	}, [pinnedDiff, onToggle])

	return (
		<WorkerPoolContextProvider
			poolOptions={WORKER_POOL_OPTIONS}
			highlighterOptions={highlighterOptions}
		>
			<DiffThemeSyncer />
			<div className="relative h-full">
				{/* Pinned file header -- shown when an expanded file's header scrolls past */}
				{pinnedDiff && (
					<div
						key={pinnedDiff.file}
						className="absolute inset-x-0 top-0 z-10 animate-in fade-in duration-150 border-b border-border/50 bg-background/60 shadow-sm backdrop-blur-md"
					>
						<FileDiffHeader
							file={pinnedDiff.file}
							additions={pinnedDiff.additions}
							deletions={pinnedDiff.deletions}
							status={pinnedDiff.status}
							collapsed={false}
							onToggle={handlePinnedToggle}
							isGenerated={isGeneratedFile(pinnedDiff.file)}
						/>
					</div>
				)}
				<div ref={scrollRef} className="h-full overflow-auto">
					<div
						style={{
							height: `${virtualizer.getTotalSize()}px`,
							width: "100%",
							position: "relative",
						}}
					>
						{virtualizer.getVirtualItems().map((virtualRow) => {
							const diff = diffs[virtualRow.index]
							const collapsed = getIsCollapsed(diff)
							return (
								<div
									key={diff.file}
									data-index={virtualRow.index}
									ref={virtualizer.measureElement}
									style={{
										position: "absolute",
										top: 0,
										left: 0,
										width: "100%",
										transform: `translateY(${virtualRow.start}px)`,
									}}
								>
									<FileDiffSection
										diff={diff}
										diffStyle={diffStyle}
										collapsed={collapsed}
										onToggle={onToggle}
										onAddComment={onAddComment}
									/>
								</div>
							)
						})}
					</div>
				</div>
			</div>
		</WorkerPoolContextProvider>
	)
})

// ============================================================
// Worker pool theme syncer
// ============================================================

/**
 * Tiny component that syncs the active theme to the worker pool when it changes.
 * Lives inside WorkerPoolContextProvider so it can call useWorkerPool().
 */
function DiffThemeSyncer() {
	const pool = useWorkerPool()
	const isDark = useIsDarkMode()
	const prevThemeRef = useRef<string | null>(null)

	useEffect(() => {
		if (!pool) return
		const theme = isDark ? "one-dark-pro" : "one-light"
		if (prevThemeRef.current === theme) return
		prevThemeRef.current = theme
		pool.setRenderOptions({ theme })
	}, [pool, isDark])

	return null
}

// ============================================================
// File list
// ============================================================

const FileList = memo(function FileList({
	diffs,
	selectedFile,
	onSelectFile,
}: {
	diffs: FileDiff[]
	selectedFile: string | null
	onSelectFile: (file: string | null) => void
}) {
	return (
		<div className="max-h-32 overflow-auto px-1 py-1">
			<button
				type="button"
				onClick={() => onSelectFile(null)}
				className={cn(
					"flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-[11px] transition-colors",
					selectedFile === null
						? "bg-muted text-foreground"
						: "text-muted-foreground hover:bg-muted/50 hover:text-foreground",
				)}
			>
				<span className="font-medium">All files</span>
				<span className="ml-auto text-[10px] text-muted-foreground/70">{diffs.length}</span>
			</button>
			{diffs.map((diff) => (
				<FileListItem
					key={diff.file}
					file={diff.file}
					additions={diff.additions}
					deletions={diff.deletions}
					isSelected={selectedFile === diff.file}
					isLarge={isLargeDiff(diff)}
					isGenerated={isGeneratedFile(diff.file)}
					onSelect={onSelectFile}
				/>
			))}
		</div>
	)
})

const FileListItem = memo(function FileListItem({
	file,
	additions,
	deletions,
	isSelected,
	isLarge,
	isGenerated,
	onSelect,
}: {
	file: string
	additions: number
	deletions: number
	isSelected: boolean
	isLarge: boolean
	isGenerated: boolean
	onSelect: (file: string | null) => void
}) {
	const handleClick = useCallback(
		() => onSelect(isSelected ? null : file),
		[file, isSelected, onSelect],
	)
	return (
		<button
			type="button"
			onClick={handleClick}
			className={cn(
				"flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-[11px] transition-colors",
				isSelected
					? "bg-muted text-foreground"
					: isGenerated
						? "text-muted-foreground/60 hover:bg-muted/50 hover:text-foreground"
						: "text-muted-foreground hover:bg-muted/50 hover:text-foreground",
			)}
		>
			<FileIcon className="size-3 shrink-0" aria-hidden="true" />
			<span className={cn("min-w-0 flex-1 truncate font-mono", isGenerated && "italic")}>
				{file}
			</span>
			<span className="flex shrink-0 items-center gap-1.5 text-[10px]">
				{isGenerated && (
					<span className="rounded bg-muted px-1 py-0.5 text-[9px] font-medium leading-none text-muted-foreground/60">
						generated
					</span>
				)}
				{isLarge && (
					<span title="Large diff">
						<AlertTriangleIcon className="size-3 text-amber-500" aria-hidden="true" />
					</span>
				)}
				{additions > 0 && <span className="text-green-500">+{additions}</span>}
				{deletions > 0 && <span className="text-red-500">-{deletions}</span>}
			</span>
		</button>
	)
})

// ============================================================
// Per-file diff section
// ============================================================

interface FileDiffSectionProps {
	diff: FileDiff
	diffStyle: DiffStyle
	collapsed: boolean
	onToggle: (file: string) => void
	onAddComment: (comment: {
		filePath: string
		lineNumber: number
		side: "additions" | "deletions"
		content: string
	}) => void
}

const FileDiffSection = memo(function FileDiffSection({
	diff,
	diffStyle,
	collapsed,
	onToggle,
	onAddComment,
}: FileDiffSectionProps) {
	const generated = isGeneratedFile(diff.file)
	const large = isLargeDiff(diff)
	const [loadLargeDiff, setLoadLargeDiff] = useState(!large)

	// Per-component options (only non-pool-controlled settings).
	// theme and lineDiffType are managed by the WorkerPoolManager.
	const options = useMemo(
		() => ({
			diffStyle: diffStyle as "unified" | "split",
			disableFileHeader: true,
			expandUnchanged: false,
		}),
		[diffStyle],
	)

	const oldFile = useMemo(
		() => ({ name: diff.file, contents: diff.before }),
		[diff.file, diff.before],
	)
	const newFile = useMemo(
		() => ({ name: diff.file, contents: diff.after }),
		[diff.file, diff.after],
	)

	const renderHoverUtility = useCallback(
		(getHoveredLine: () => { lineNumber: number; side: "additions" | "deletions" } | undefined) => (
			<DiffCommentButton
				filePath={diff.file}
				getHoveredLine={getHoveredLine}
				onAddComment={onAddComment}
			/>
		),
		[diff.file, onAddComment],
	)

	const handleToggle = useCallback(() => onToggle(diff.file), [diff.file, onToggle])
	const handleLoadLarge = useCallback(() => {
		startTransition(() => setLoadLargeDiff(true))
	}, [])

	// Determine what body content to show
	let body: ReactNode = null
	if (!collapsed) {
		if (!loadLargeDiff) {
			body = (
				<LargeDiffPlaceholder
					additions={diff.additions}
					deletions={diff.deletions}
					onLoad={handleLoadLarge}
				/>
			)
		} else {
			// Worker pool renders plain text synchronously, then streams in
			// syntax highlighting from the background -- no manual queue needed.
			body = (
				<div className="overflow-x-auto">
					<MultiFileDiff
						options={options}
						oldFile={oldFile}
						newFile={newFile}
							renderGutterUtility={renderHoverUtility}
					/>
				</div>
			)
		}
	}

	return (
		<div className="border-b border-border last:border-b-0">
			<FileDiffHeader
				file={diff.file}
				additions={diff.additions}
				deletions={diff.deletions}
				status={diff.status}
				collapsed={collapsed}
				onToggle={handleToggle}
				isLarge={large && !loadLargeDiff}
				isGenerated={generated}
			/>
			{body}
		</div>
	)
})

// ============================================================
// Large diff placeholder
// ============================================================

function LargeDiffPlaceholder({
	additions,
	deletions,
	onLoad,
}: {
	additions: number
	deletions: number
	onLoad: () => void
}) {
	const totalLines = additions + deletions
	return (
		<div className="flex flex-col items-center justify-center gap-2 bg-muted/10 px-4 py-6">
			<div className="flex items-center gap-1.5 text-xs text-amber-500">
				<AlertTriangleIcon className="size-3.5" />
				<span>Large diff ({totalLines.toLocaleString()} lines changed) not shown</span>
			</div>
			<button
				type="button"
				onClick={onLoad}
				className="rounded-md border border-border bg-background px-3 py-1 text-xs text-foreground transition-colors hover:bg-muted"
			>
				Load diff
			</button>
		</div>
	)
}

// ============================================================
// File diff header
// ============================================================

const FileDiffHeader = memo(function FileDiffHeader({
	file,
	additions,
	deletions,
	status,
	collapsed,
	onToggle,
	isLarge,
	isGenerated,
	loading,
}: {
	file: string
	additions: number
	deletions: number
	status?: "added" | "deleted" | "modified"
	collapsed: boolean
	onToggle: () => void
	isLarge?: boolean
	isGenerated?: boolean
	loading?: boolean
}) {
	return (
		<button
			type="button"
			onClick={onToggle}
			className="flex w-full items-center gap-2 bg-muted/30 px-3 py-1.5 text-left transition-colors hover:bg-muted/50"
		>
			{loading ? (
				<Loader2Icon className="size-3 shrink-0 animate-spin text-muted-foreground" />
			) : collapsed ? (
				<ChevronRightIcon className="size-3 shrink-0 text-muted-foreground" />
			) : (
				<ChevronDownIcon className="size-3 shrink-0 text-muted-foreground" />
			)}
			<span
				className={cn(
					"min-w-0 flex-1 truncate font-mono text-xs",
					isGenerated ? "italic text-muted-foreground" : "text-foreground",
				)}
			>
				{file}
			</span>
			<span className="flex shrink-0 items-center gap-1.5 text-[11px]">
				{isGenerated && (
					<span className="rounded bg-muted px-1 py-0.5 text-[9px] font-medium leading-none text-muted-foreground/60">
						generated
					</span>
				)}
				{isLarge && (
					<span className="rounded bg-amber-500/15 px-1 py-0.5 text-[9px] font-medium leading-none text-amber-500">
						LARGE
					</span>
				)}
				<span className="flex items-center gap-0.5 text-green-500">
					<PlusIcon className="size-2.5" aria-hidden="true" />
					{additions}
				</span>
				<span className="flex items-center gap-0.5 text-red-500">
					<MinusIcon className="size-2.5" aria-hidden="true" />
					{deletions}
				</span>
				{status && <StatusBadge status={status} />}
			</span>
		</button>
	)
})

// ============================================================
// Status badge
// ============================================================

const STATUS_CONFIG = {
	added: { label: "A", className: "bg-green-500/15 text-green-500" },
	deleted: { label: "D", className: "bg-red-500/15 text-red-500" },
	modified: { label: "M", className: "bg-blue-500/15 text-blue-500" },
} as const

function StatusBadge({ status }: { status: "added" | "deleted" | "modified" }) {
	const c = STATUS_CONFIG[status]
	return (
		<span
			className={cn(
				"inline-flex size-4 items-center justify-center rounded text-[10px] font-bold leading-none",
				c.className,
			)}
		>
			{c.label}
		</span>
	)
}

// ============================================================
// Empty state
// ============================================================

function EmptyState() {
	return (
		<div className="flex flex-col items-center justify-center gap-3 py-16">
			<div className="flex size-10 items-center justify-center rounded-lg border border-border/50 bg-muted/30">
				<FileIcon className="size-4 text-muted-foreground" />
			</div>
			<div className="text-center">
				<p className="text-sm font-medium text-foreground">No changes yet</p>
				<p className="mt-1 text-xs text-muted-foreground">
					File changes will appear here as the agent works
				</p>
			</div>
		</div>
	)
}
