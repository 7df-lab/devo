/**
 * Worktree action buttons for the session app bar.
 *
 * Shows "Apply to project" and "Commit & Push" actions for worktree sessions.
 * "Apply to project" patches the worktree's uncommitted changes into the main
 * project checkout (the parent directory of the worktree).
 * "Commit & Push" commits all changes and pushes the branch to origin.
 */

import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Textarea } from "@devo/ui/components/textarea"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import {
	ArrowDownToLineIcon,
	ArrowUpFromLineIcon,
	CheckIcon,
	GitBranchIcon,
	GitCommitHorizontalIcon,
	Loader2Icon,
	XIcon,
} from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import type { GitDiffStat } from "../../preload/api"
import type { Agent } from "../lib/types"
import {
	fetchDiffStat,
	getGitRemoteUrl,
	gitApplyToLocal,
	gitCommitAll,
	gitCreateBranch,
	gitPush,
	isElectron,
} from "../services/backend"

// ============================================================
// Types
// ============================================================

interface WorktreeActionsProps {
	agent: Agent
}

type CommitStep = "commit" | "commit-push" | "commit-push-pr"

// ============================================================
// Main component
// ============================================================

export function WorktreeActions({ agent }: WorktreeActionsProps) {
	if (!agent.worktreePath) return null

	return (
		<div className="flex items-center gap-1">
			<ApplyToLocalButton agent={agent} />
			<CommitPushButton agent={agent} />
		</div>
	)
}

// ============================================================
// Apply to Local
// ============================================================

function ApplyToLocalButton({ agent }: { agent: Agent }) {
	const [loading, setLoading] = useState(false)
	const [result, setResult] = useState<{ success: boolean; message: string } | null>(null)

	// Applying requires Electron (for the local `git apply` step).
	const canApply = isElectron

	// The target for apply is always the main project directory, not the worktree.
	const targetDir = agent.projectDirectory

	const handleApply = useCallback(async () => {
		if (!agent.worktreePath || !canApply) return
		setLoading(true)
		setResult(null)
		try {
			const res = await gitApplyToLocal(agent.worktreePath, targetDir)
			if (res.success) {
				setResult({
					success: true,
					message: `Applied ${res.filesApplied.length} file${res.filesApplied.length !== 1 ? "s" : ""} to project`,
				})
			} else {
				setResult({ success: false, message: res.error ?? "Apply failed" })
			}
		} catch (err) {
			setResult({
				success: false,
				message: err instanceof Error ? err.message : "Apply failed",
			})
		} finally {
			setLoading(false)
			setTimeout(() => setResult(null), 4000)
		}
	}, [agent.worktreePath, targetDir])

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<Button
						size="sm"
						variant="ghost"
						className="h-7 gap-1.5 px-2 text-xs text-muted-foreground hover:text-foreground"
						onClick={handleApply}
						disabled={loading || !canApply}
					/>
				}
			>
				{loading ? (
					<Loader2Icon className="size-3 animate-spin" />
				) : result ? (
					result.success ? (
						<CheckIcon className="size-3 text-green-500" />
					) : (
						<XIcon className="size-3 text-red-500" />
					)
				) : (
					<ArrowDownToLineIcon className="size-3" />
				)}
				{result ? (
					<span className={result.success ? "text-green-500" : "text-red-500"}>
						{result.message}
					</span>
				) : (
					"Apply to project"
				)}
			</TooltipTrigger>
			<TooltipContent side="bottom">
				{canApply
					? "Apply worktree changes to your project as uncommitted changes"
					: "Apply to project requires the Electron desktop app"}
			</TooltipContent>
		</Tooltip>
	)
}

// ============================================================
// Commit & Push (dialog)
// ============================================================

function CommitPushButton({ agent }: { agent: Agent }) {
	const [open, setOpen] = useState(false)

	if (!agent.worktreePath) return null

	return (
		<>
			<Tooltip>
				<TooltipTrigger
					render={
						<Button
							size="sm"
							variant="ghost"
							className="h-7 gap-1.5 px-2 text-xs text-muted-foreground hover:text-foreground"
							onClick={() => setOpen(true)}
						/>
					}
				>
					<ArrowUpFromLineIcon className="size-3" />
					Commit & push
				</TooltipTrigger>
				<TooltipContent side="bottom">Commit all changes and push to remote</TooltipContent>
			</Tooltip>

			<CommitDialog open={open} onOpenChange={setOpen} agent={agent} />
		</>
	)
}

// ============================================================
// Commit Dialog
// ============================================================

function CommitDialog({
	open,
	onOpenChange,
	agent,
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	agent: Agent
}) {
	const [diffStat, setDiffStat] = useState<GitDiffStat | null>(null)
	const [loadingDiff, setLoadingDiff] = useState(false)
	const [commitMessage, setCommitMessage] = useState("")
	const [step, setStep] = useState<CommitStep>("commit-push")
	const [executing, setExecuting] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [success, setSuccess] = useState<string | null>(null)

	// Ensure we have a branch name. If worktreeBranch is set, use it.
	// Otherwise the user needs to create one.
	const [branchName, setBranchName] = useState(agent.worktreeBranch ?? "")
	const hasBranch = !!agent.worktreeBranch

	// Load diff stat when dialog opens
	useEffect(() => {
		if (!open || !agent.worktreePath) return
		setLoadingDiff(true)
		setError(null)
		setSuccess(null)
		fetchDiffStat(agent.worktreePath)
			.then(setDiffStat)
			.catch(() => setDiffStat(null))
			.finally(() => setLoadingDiff(false))
	}, [open, agent.worktreePath])

	const handleExecute = useCallback(async () => {
		if (!agent.worktreePath) return
		setExecuting(true)
		setError(null)

		try {
			// Step 1: Create branch if we don't have one yet
			if (!hasBranch && branchName) {
				const branchResult = await gitCreateBranch(agent.worktreePath, branchName)
				if (!branchResult.success) {
					setError(`Branch creation failed: ${branchResult.error}`)
					return
				}
			}

			// Step 2: Commit all changes
			const msg =
				commitMessage.trim() ||
				`Update ${diffStat?.filesChanged || 0} file${diffStat?.filesChanged !== 1 ? "s" : ""}`
			const commitResult = await gitCommitAll(agent.worktreePath, msg)
			if (!commitResult.success) {
				setError(`Commit failed: ${commitResult.error}`)
				return
			}

			// Step 3: Push if requested
			if (step === "commit-push" || step === "commit-push-pr") {
				const pushResult = await gitPush(agent.worktreePath)
				if (!pushResult.success) {
					setError(`Push failed: ${pushResult.error}`)
					return
				}
			}

			// Step 4: Open PR URL if requested
			if (step === "commit-push-pr" && agent.worktreePath) {
				// Construct a GitHub new-PR URL. Best-effort for GitHub repos.
				try {
					const effectiveBranch = branchName || agent.worktreeBranch || ""
					if (effectiveBranch) {
						const remoteUrl = await getGitRemoteUrl(agent.worktreePath)
						if (remoteUrl) {
							const match = remoteUrl.match(/(?:github\.com)[/:](.+?)(?:\.git)?$/)
							if (match) {
								const repoPath = match[1]
								const prUrl = `https://github.com/${repoPath}/compare/${effectiveBranch}?expand=1`
								window.open(prUrl, "_blank")
							}
						}
					}
				} catch {
					// Best effort - PR URL construction failed, but commit+push succeeded
				}
			}

			setSuccess(
				step === "commit"
					? "Committed successfully"
					: step === "commit-push"
						? "Committed and pushed"
						: "Committed, pushed, and PR page opened",
			)
			setTimeout(() => onOpenChange(false), 1500)
		} catch (err) {
			setError(err instanceof Error ? err.message : "Operation failed")
		} finally {
			setExecuting(false)
		}
	}, [
		agent.worktreePath,
		agent.worktreeBranch,
		branchName,
		commitMessage,
		step,
		hasBranch,
		onOpenChange,
		diffStat?.filesChanged,
	])

	const stepLabels: Record<CommitStep, { label: string; icon: typeof GitCommitHorizontalIcon }> = {
		commit: { label: "Commit", icon: GitCommitHorizontalIcon },
		"commit-push": { label: "Commit & push", icon: ArrowUpFromLineIcon },
		"commit-push-pr": { label: "Commit, push & create PR", icon: GitBranchIcon },
	}

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="flex max-h-[85vh] max-w-md flex-col">
				<DialogHeader className="shrink-0">
					<DialogTitle className="flex items-center gap-2">
						<GitCommitHorizontalIcon className="size-5" />
						Commit your changes
					</DialogTitle>
					<DialogDescription>
						Commit changes from the worktree and optionally push or create a PR.
					</DialogDescription>
				</DialogHeader>

				<div className="min-h-0 flex-1 space-y-4 overflow-y-auto pr-1">
					{/* Branch */}
					<div className="space-y-1.5">
						<div className="text-sm font-medium">Branch</div>
						{hasBranch ? (
							<div className="flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-sm">
								<GitBranchIcon className="size-3.5 text-muted-foreground" />
								<span className="truncate">{agent.worktreeBranch}</span>
							</div>
						) : (
							<Input
								value={branchName}
								onChange={(e) => setBranchName(e.target.value)}
								placeholder="devo/feature-name"
								className="text-sm"
							/>
						)}
					</div>

					{/* Diff stat summary */}
					<div className="space-y-1.5">
						<div className="text-sm font-medium">Changes</div>
						{loadingDiff ? (
							<div className="flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-sm text-muted-foreground">
								<Loader2Icon className="size-3.5 animate-spin" />
								Scanning changes...
							</div>
						) : diffStat ? (
							<div className="rounded-md bg-muted px-3 py-2">
								<div className="text-sm">
									{diffStat.filesChanged} file{diffStat.filesChanged !== 1 ? "s" : ""} changed
								</div>
								{diffStat.files.length > 0 && (
									<div className="mt-1.5 max-h-[120px] space-y-0.5 overflow-y-auto">
										{diffStat.files.map((f) => (
											<div
												key={f.path}
												className="truncate font-mono text-[11px] text-muted-foreground"
											>
												{f.path}
											</div>
										))}
									</div>
								)}
							</div>
						) : (
							<div className="rounded-md bg-muted px-3 py-2 text-sm text-muted-foreground">
								No changes detected
							</div>
						)}
					</div>

					{/* Commit message */}
					<div className="space-y-1.5">
						<div className="text-sm font-medium">Commit message</div>
						<Textarea
							value={commitMessage}
							onChange={(e) => setCommitMessage(e.target.value)}
							placeholder="Describe your changes (optional)"
							className="min-h-[60px] resize-none text-sm"
						/>
					</div>

					{/* Action selector */}
					<div className="space-y-1.5">
						<div className="text-sm font-medium">Action</div>
						<div className="divide-y divide-border rounded-md border border-border">
							{(
								Object.entries(stepLabels) as [
									CommitStep,
									{ label: string; icon: typeof GitCommitHorizontalIcon },
								][]
							).map(([key, { label, icon: Icon }]) => (
								<button
									key={key}
									type="button"
									onClick={() => setStep(key)}
									className={`flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm transition-colors ${
										step === key
											? "bg-accent/50 text-foreground"
											: "text-muted-foreground hover:bg-muted/50 hover:text-foreground"
									}`}
								>
									<Icon className="size-4 shrink-0" />
									<span className="flex-1">{label}</span>
									{step === key && <CheckIcon className="size-3.5 shrink-0 text-primary" />}
								</button>
							))}
						</div>
					</div>

					{/* Error / success */}
					{error && (
						<div className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
							{error}
						</div>
					)}
					{success && (
						<div className="rounded-md bg-green-500/10 px-3 py-2 text-sm text-green-600">
							{success}
						</div>
					)}
				</div>

				<DialogFooter className="shrink-0">
					<Button variant="outline" onClick={() => onOpenChange(false)} disabled={executing}>
						Cancel
					</Button>
					<Button
						onClick={handleExecute}
						disabled={executing || (!hasBranch && !branchName) || !diffStat?.filesChanged}
					>
						{executing ? (
							<>
								<Loader2Icon className="size-3.5 animate-spin" />
								Working...
							</>
						) : (
							stepLabels[step].label
						)}
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
