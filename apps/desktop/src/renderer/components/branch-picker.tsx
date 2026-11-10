import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import {
	SearchableListPopover,
	SearchableListPopoverContent,
	SearchableListPopoverEmpty,
	SearchableListPopoverGroup,
	SearchableListPopoverItem,
	SearchableListPopoverList,
	SearchableListPopoverSearch,
	SearchableListPopoverTrigger,
	useSearchableListPopoverSearch,
} from "@devo/ui/components/searchable-list-popover"
import {
	AlertTriangleIcon,
	CheckIcon,
	ChevronsUpDownIcon,
	GitBranchIcon,
	GlobeIcon,
	Loader2Icon,
	SearchIcon,
} from "lucide-react"
import { useMemo } from "react"
import type { GitBranchInfo, GitBranchState, GitStatusInfo } from "../../preload/api"
import { isElectron } from "../services/backend"
import { useBranchPicker } from "./use-branch-picker"

// ============================================================
// Types
// ============================================================

interface BranchPickerProps {
	/** Project directory to operate on */
	directory: string
	/** Current branch from VCS (display-only fallback) */
	currentBranch?: string
	/** Current git state from VCS (display-only fallback) */
	currentState?: GitBranchState
	/** Called after a successful branch switch */
	onBranchChanged?: (branch: string) => void
	/** Number of active sessions on this directory (for warnings) */
	activeSessionCount?: number
}

// ============================================================
// Branch Picker
// ============================================================

export function BranchPicker(props: BranchPickerProps) {
	return <ProjectBranchPicker key={props.directory} {...props} />
}

function ProjectBranchPicker({
	directory,
	currentBranch,
	currentState,
	onBranchChanged,
	activeSessionCount = 0,
}: BranchPickerProps) {
	const {
		open,
		setOpen,
		branches,
		loading,
		loadError,
		retryLoad,
		operation,
		switchError,
		dismissError,
		selectBranch,
		stashAndSwitch,
		proceed,
		cancelDirty,
		cancelWarning,
	} = useBranchPicker({
		directory,
		currentBranch,
		activeSessionCount,
		onBranchChanged,
	})
	const busy = operation?.phase === "checking" || operation?.phase === "switching"
	const effectiveCurrent = branches?.current || currentBranch || ""
	const effectiveState =
		branches?.state ?? currentState ?? (effectiveCurrent ? "branch" : undefined)
	const displayLabel = branchTriggerLabel(effectiveState, effectiveCurrent)

	// Don't render in browser mode
	if (!isElectron) return null

	return (
		<>
			<SearchableListPopover open={open} onOpenChange={setOpen}>
				<SearchableListPopoverTrigger
					render={
						<button
							type="button"
							disabled={operation !== null}
							className="flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
						/>
					}
				>
					{busy ? (
						<Loader2Icon className="size-3 animate-spin" />
					) : (
						<GitBranchIcon className="size-3" />
					)}
					<span className="max-w-[140px] truncate">
						{busy
							? operation.phase === "checking"
								? "Checking..."
								: "Switching..."
							: displayLabel}
					</span>
					<ChevronsUpDownIcon className="size-3 opacity-50" />
				</SearchableListPopoverTrigger>
				<SearchableListPopoverContent align="end" side="top">
					<SearchableListPopoverSearch
						placeholder="Search branches..."
						icon={<SearchIcon className="size-4" />}
					/>

					{loading ? (
						<div className="flex items-center justify-center py-6">
							<Loader2Icon className="size-4 animate-spin text-muted-foreground" />
						</div>
					) : loadError ? (
						<div className="space-y-2 p-3 text-sm">
							<p role="alert" className="text-destructive">
								{loadError}
							</p>
							<Button variant="outline" size="sm" onClick={retryLoad}>
								Retry
							</Button>
						</div>
					) : (
						<BranchList
							branches={branches}
							effectiveCurrent={effectiveCurrent}
							onSelectBranch={selectBranch}
						/>
					)}
				</SearchableListPopoverContent>
			</SearchableListPopover>

			{/* Error toast */}
			{switchError && (
				<div
					role="alert"
					className="fixed right-4 bottom-4 z-50 max-w-sm rounded-lg border border-red-500/20 bg-red-500/10 px-4 py-3 text-sm text-red-500 shadow-lg"
				>
					<div className="flex items-start gap-2">
						<AlertTriangleIcon className="mt-0.5 size-4 shrink-0" />
						<div>
							<p className="font-medium">Branch switch failed</p>
							<p className="mt-1 text-xs opacity-80">{switchError}</p>
						</div>
						<button
							type="button"
							onClick={dismissError}
							className="ml-auto text-xs hover:text-red-300"
						>
							Dismiss
						</button>
					</div>
				</div>
			)}

			{/* Dirty state dialog */}
			<DirtyStateDialog
				open={operation?.phase === "dirty"}
				status={operation?.status ?? null}
				targetBranch={operation?.branch ?? ""}
				onStashAndSwitch={stashAndSwitch}
				onCancel={cancelDirty}
			/>

			{/* Active session warning dialog */}
			<ActiveSessionWarningDialog
				open={operation?.phase === "warning"}
				targetBranch={operation?.branch ?? ""}
				sessionCount={activeSessionCount}
				onProceed={proceed}
				onCancel={cancelWarning}
			/>
		</>
	)
}

function branchTriggerLabel(state: GitBranchState | undefined, current: string): string {
	if (!state) return current || "branch unavailable"
	switch (state) {
		case "branch":
			return current || "branch unavailable"
		case "detached":
			return current ? `detached: ${current}` : "detached HEAD"
		case "missing":
			return "missing project"
		case "not_directory":
			return "not a directory"
		case "not_git":
			return "not a git repo"
	}
}

// ============================================================
// Branch list — reads search from context
// ============================================================

function BranchList({
	branches,
	effectiveCurrent,
	onSelectBranch,
}: {
	branches: GitBranchInfo | null
	effectiveCurrent: string
	onSelectBranch: (branch: string) => void
}) {
	const search = useSearchableListPopoverSearch()
	const unavailableMessage = branches ? unavailableBranchMessage(branches.state) : null

	const filteredLocal = useMemo(() => {
		if (!branches) return []
		const term = search.toLowerCase()
		return branches.local.filter((b: string) => b.toLowerCase().includes(term))
	}, [branches, search])

	const filteredRemote = useMemo(() => {
		if (!branches) return []
		const term = search.toLowerCase()
		const localSet = new Set(branches.local)
		return branches.remote
			.filter((b: string) => {
				const localName = b.replace(/^[^/]+\//, "")
				return !localSet.has(localName)
			})
			.filter((b: string) => b.toLowerCase().includes(term))
	}, [branches, search])

	return (
		<SearchableListPopoverList maxHeight="max-h-[300px]">
			{unavailableMessage && (
				<SearchableListPopoverEmpty>{unavailableMessage}</SearchableListPopoverEmpty>
			)}

			{!unavailableMessage && filteredLocal.length > 0 && (
				<SearchableListPopoverGroup label="Local">
					{filteredLocal.map((branch: string) => (
						<BranchItem
							key={branch}
							name={branch}
							isCurrent={branch === effectiveCurrent}
							onSelect={() => onSelectBranch(branch)}
						/>
					))}
				</SearchableListPopoverGroup>
			)}

			{!unavailableMessage && filteredRemote.length > 0 && (
				<SearchableListPopoverGroup label="Remote">
					{filteredRemote.map((branch: string) => (
						<BranchItem
							key={branch}
							name={branch}
							isCurrent={false}
							isRemote
							onSelect={() => onSelectBranch(branch)}
						/>
					))}
				</SearchableListPopoverGroup>
			)}

			{!unavailableMessage && filteredLocal.length === 0 && filteredRemote.length === 0 && (
				<SearchableListPopoverEmpty>
					{search ? "No matching branches" : "No branches found"}
				</SearchableListPopoverEmpty>
			)}
		</SearchableListPopoverList>
	)
}

function unavailableBranchMessage(state: GitBranchState): string | null {
	switch (state) {
		case "branch":
		case "detached":
			return null
		case "missing":
			return "Project directory is missing"
		case "not_directory":
			return "Project path is not a directory"
		case "not_git":
			return "Project is not a git repository"
	}
}

// ============================================================
// Branch list item
// ============================================================

function BranchItem({
	name,
	isCurrent,
	isRemote,
	onSelect,
}: {
	name: string
	isCurrent: boolean
	isRemote?: boolean
	onSelect: () => void
}) {
	return (
		<SearchableListPopoverItem onSelect={onSelect} isActive={isCurrent}>
			{isCurrent ? (
				<CheckIcon className="size-3.5 shrink-0 text-green-500" />
			) : isRemote ? (
				<GlobeIcon className="size-3.5 shrink-0 text-muted-foreground" />
			) : (
				<GitBranchIcon className="size-3.5 shrink-0 text-muted-foreground" />
			)}
			<span className="min-w-0 flex-1 truncate">{name}</span>
			{isCurrent && <span className="shrink-0 text-[10px] text-muted-foreground">current</span>}
		</SearchableListPopoverItem>
	)
}

// ============================================================
// Dirty State Dialog
// ============================================================

interface DirtyStateDialogProps {
	open: boolean
	status: GitStatusInfo | null
	targetBranch: string
	onStashAndSwitch: () => void
	onCancel: () => void
}

function DirtyStateDialog({
	open,
	status,
	targetBranch,
	onStashAndSwitch,
	onCancel,
}: DirtyStateDialogProps) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !isOpen && onCancel()}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<DialogHeader>
					<DialogTitle className="flex items-center gap-2">
						<AlertTriangleIcon className="size-5 text-yellow-500" />
						Uncommitted Changes
					</DialogTitle>
					<DialogDescription>
						You have uncommitted changes that need to be handled before switching to{" "}
						<code className="rounded bg-muted px-1.5 py-0.5 text-foreground">{targetBranch}</code>.
					</DialogDescription>
				</DialogHeader>

				{/* Status summary */}
				{status && (
					<div className="rounded-md border bg-muted/50 px-3 py-2">
						<div className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
							{status.staged > 0 && <span className="text-green-500">{status.staged} staged</span>}
							{status.modified > 0 && (
								<span className="text-yellow-500">{status.modified} modified</span>
							)}
							{status.untracked > 0 && (
								<span className="text-muted-foreground">{status.untracked} untracked</span>
							)}
							{status.conflicted > 0 && (
								<span className="text-red-500">{status.conflicted} conflicted</span>
							)}
						</div>
					</div>
				)}

				<DialogFooter>
					<Button variant="outline" onClick={onCancel} disabled={!open}>
						Cancel
					</Button>
					<Button onClick={onStashAndSwitch} disabled={!open}>
						Stash & Switch
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}

// ============================================================
// Active Session Warning Dialog
// ============================================================

interface ActiveSessionWarningDialogProps {
	open: boolean
	targetBranch: string
	sessionCount: number
	onProceed: () => void
	onCancel: () => void
}

function ActiveSessionWarningDialog({
	open,
	targetBranch,
	sessionCount,
	onProceed,
	onCancel,
}: ActiveSessionWarningDialogProps) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !isOpen && onCancel()}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<DialogHeader>
					<DialogTitle className="flex items-center gap-2">
						<AlertTriangleIcon className="size-5 text-orange-500" />
						Active Sessions
					</DialogTitle>
					<DialogDescription>
						There {sessionCount === 1 ? "is" : "are"} <strong>{sessionCount}</strong> active{" "}
						{sessionCount === 1 ? "session" : "sessions"} on this project. Switching to{" "}
						<code className="rounded bg-muted px-1.5 py-0.5 text-foreground">{targetBranch}</code>{" "}
						will change the working directory for all of them.
					</DialogDescription>
				</DialogHeader>

				<DialogFooter>
					<Button variant="outline" onClick={onCancel} disabled={!open}>
						Cancel
					</Button>
					<Button variant="destructive" onClick={onProceed} disabled={!open}>
						Switch Anyway
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
