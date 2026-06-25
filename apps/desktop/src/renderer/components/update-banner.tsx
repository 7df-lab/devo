import {
	ArrowDownToLineIcon,
	ExternalLinkIcon,
	RefreshCwIcon,
	SparklesIcon,
	XIcon,
} from "lucide-react"
import { useState } from "react"
import { useUpdater } from "../hooks/use-updater"

/**
 * Floating toast-style update notification that appears in the bottom-right
 * corner when a new version is available, downloading, or ready to install.
 *
 * Overlays content instead of pushing it down. Hidden when idle, checking,
 * or dismissed by the user.
 *
 * On unsigned macOS builds (canAutoInstall=false), the "ready" state shows
 * a "Download from GitHub" button that opens the release page instead of
 * attempting an in-place install via Squirrel.Mac.
 */
export function UpdateBanner() {
	const { status, version, progress, canAutoInstall, downloadUpdate, installUpdate, openReleasePage } =
		useUpdater()
	const [dismissed, setDismissed] = useState(false)

	// Don't show for idle/checking/error states, or if dismissed
	if (dismissed || status === "idle" || status === "checking" || status === "error") {
		return null
	}

	return (
		<div className="fixed right-4 bottom-4 z-50 w-72 animate-in fade-in slide-in-from-bottom-2 duration-300">
			<div className="rounded-xl border border-border bg-popover p-3.5 shadow-lg">
				{status === "available" && (
					<>
						<div className="mb-3 flex items-start justify-between gap-2">
							<div className="flex items-start gap-2.5">
								<div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10">
									<SparklesIcon className="size-4 text-primary" aria-hidden="true" />
								</div>
								<div className="min-w-0">
									<p className="text-sm font-medium leading-tight">Update available</p>
									<p className="mt-0.5 text-xs text-muted-foreground">
										{version ? `Version ${version}` : "A new version"} is ready to download
									</p>
								</div>
							</div>
							<button
								type="button"
								onClick={() => setDismissed(true)}
								className="-m-1 rounded-md p-1 text-muted-foreground transition-colors hover:text-foreground"
								aria-label="Dismiss update notification"
							>
								<XIcon className="size-3.5" aria-hidden="true" />
							</button>
						</div>
						{canAutoInstall ? (
							<button
								type="button"
								onClick={() => downloadUpdate()}
								className="flex w-full items-center justify-center gap-1.5 rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
							>
								<ArrowDownToLineIcon className="size-3.5" aria-hidden="true" />
								Download update
							</button>
						) : (
							<button
								type="button"
								onClick={() => openReleasePage()}
								className="flex w-full items-center justify-center gap-1.5 rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
							>
								<ExternalLinkIcon className="size-3.5" aria-hidden="true" />
								Download from GitHub
							</button>
						)}
					</>
				)}

				{status === "downloading" && (
					<>
						<div className="mb-3 flex items-start gap-2.5">
							<div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10">
								<ArrowDownToLineIcon
									className="size-4 animate-pulse text-primary"
									aria-hidden="true"
								/>
							</div>
							<div className="min-w-0">
								<p className="text-sm font-medium leading-tight">
									Downloading{version ? ` v${version}` : ""}
								</p>
								<p className="mt-0.5 text-xs text-muted-foreground">
									{progress ? `${Math.round(progress.percent)}% complete` : "Starting download..."}
								</p>
							</div>
						</div>
						<div className="h-1.5 overflow-hidden rounded-full bg-muted">
							<div
								className="h-full rounded-full bg-primary transition-all duration-300"
								style={{ width: `${progress?.percent ?? 0}%` }}
							/>
						</div>
					</>
				)}

				{status === "ready" && (
					<>
						<div className="mb-3 flex items-start justify-between gap-2">
							<div className="flex items-start gap-2.5">
								<div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10">
									<RefreshCwIcon className="size-4 text-primary" aria-hidden="true" />
								</div>
								<div className="min-w-0">
									<p className="text-sm font-medium leading-tight">Ready to install</p>
									<p className="mt-0.5 text-xs text-muted-foreground">
										{canAutoInstall
											? `${version ? `Version ${version}` : "Update"} downloaded. Restart to apply.`
											: `${version ? `Version ${version}` : "Update"} is available. Download from GitHub to update.`}
									</p>
								</div>
							</div>
							<button
								type="button"
								onClick={() => setDismissed(true)}
								className="-m-1 rounded-md p-1 text-muted-foreground transition-colors hover:text-foreground"
								aria-label="Dismiss update notification"
							>
								<XIcon className="size-3.5" aria-hidden="true" />
							</button>
						</div>
						{canAutoInstall ? (
							<button
								type="button"
								onClick={() => installUpdate()}
								className="flex w-full items-center justify-center gap-1.5 rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
							>
								<RefreshCwIcon className="size-3.5" aria-hidden="true" />
								Restart now
							</button>
						) : (
							<button
								type="button"
								onClick={() => openReleasePage()}
								className="flex w-full items-center justify-center gap-1.5 rounded-lg bg-primary px-3 py-1.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
							>
								<ExternalLinkIcon className="size-3.5" aria-hidden="true" />
								Download from GitHub
							</button>
						)}
					</>
				)}
			</div>
		</div>
	)
}
