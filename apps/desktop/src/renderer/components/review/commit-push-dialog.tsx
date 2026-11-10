/**
 * CommitPushDialog -- lightweight "commit everything and push" dialog for the
 * ReviewPanel. Unlike the worktree CommitDialog there is no branch selection
 * or PR step: `git add -A` + commit + push on the session's directory, then a
 * refresh callback so the panel re-reads the (now clean) working tree.
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
import { Textarea } from "@devo/ui/components/textarea"
import { GitCommitHorizontalIcon, Loader2Icon } from "lucide-react"
import { useCommitPushDialog } from "./use-commit-push-dialog"

interface CommitPushDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	directory: string
	/** Called after a successful commit (even if the push failed) */
	onCommitted: () => void
}

export function CommitPushDialog(props: CommitPushDialogProps) {
	return <ProjectCommitPushDialog key={props.directory} {...props} />
}

function ProjectCommitPushDialog({
	open,
	onOpenChange,
	directory,
	onCommitted,
}: CommitPushDialogProps) {
	const {
		diffStat,
		loadingDiff,
		scanError,
		commitMessage,
		setCommitMessage,
		phase,
		committed,
		uncertain,
		error,
		success,
		execute,
		changeOpen,
		retryScan,
	} = useCommitPushDialog({ open, directory, onOpenChange, onCommitted })
	const executing = phase !== "idle"
	const filesChanged = diffStat?.filesChanged ?? 0
	const nothingToCommit = !loadingDiff && !!diffStat && filesChanged === 0
	const canExecute =
		open &&
		!executing &&
		!uncertain &&
		(committed || (!loadingDiff && !!diffStat && !nothingToCommit))

	return (
		<Dialog open={open} onOpenChange={changeOpen}>
			<DialogContent className="max-w-md" showCloseButton={!executing}>
				<DialogHeader>
					<DialogTitle className="flex items-center gap-2">
						<GitCommitHorizontalIcon className="size-5" />
						Commit &amp; push
					</DialogTitle>
					<DialogDescription>
						Stage all changes, commit, and push the current branch.
					</DialogDescription>
				</DialogHeader>

				<div className="space-y-4">
					<div className="rounded-md bg-muted px-3 py-2 text-sm">
						{committed ? (
							<span>Commit saved locally</span>
						) : loadingDiff ? (
							<span className="flex items-center gap-2 text-muted-foreground">
								<Loader2Icon className="size-3.5 animate-spin" />
								Scanning changes...
							</span>
						) : scanError ? (
							<div className="space-y-2">
								<p role="alert" className="text-destructive">
									{scanError}
								</p>
								<Button variant="outline" size="sm" onClick={retryScan} disabled={!open}>
									Retry scan
								</Button>
							</div>
						) : nothingToCommit ? (
							<span className="text-muted-foreground">No changes detected</span>
						) : (
							<span>
								{filesChanged} file{filesChanged !== 1 ? "s" : ""} changed
							</span>
						)}
					</div>

					<Textarea
						disabled={!open || executing || committed || uncertain}
						aria-label="Commit message"
						value={commitMessage}
						onChange={(e) => setCommitMessage(e.target.value)}
						placeholder="Commit message (optional)"
						className="min-h-[60px] resize-none text-sm"
					/>

					{error && (
						<div
							role="alert"
							className="rounded-md bg-destructive/10 px-3 py-2 text-sm break-words text-destructive"
						>
							{error}
						</div>
					)}
					{success && (
						<output className="block rounded-md bg-green-500/10 px-3 py-2 text-sm text-green-600">
							Committed and pushed
						</output>
					)}
				</div>

				<DialogFooter>
					<Button variant="outline" onClick={() => changeOpen(false)} disabled={!open || executing}>
						Cancel
					</Button>
					<Button
						onClick={success ? () => changeOpen(false) : execute}
						disabled={!open || (!success && !canExecute)}
					>
						{executing && <Loader2Icon className="size-3.5 animate-spin" />}
						{success
							? "Done"
							: phase === "committing"
								? "Committing..."
								: phase === "pushing"
									? "Pushing..."
									: committed
										? "Retry push"
										: "Commit & push"}
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
