import type { SessionResolution } from "../hooks/use-session-resolution"

/** Shared empty state for routed and embedded sessions, including cold children. */
export function SessionResolutionState({
	state,
	onRetry,
}: {
	state: SessionResolution
	onRetry: () => void
}) {
	if (state.status === "idle") return null
	if (state.status === "loading")
		return (
			<div className="flex h-full items-center justify-center">
				<output
					aria-label="Loading session"
					className="size-4 animate-spin rounded-full border-2 border-muted-foreground/20 border-t-muted-foreground/60"
				>
					<span className="sr-only">Loading session…</span>
				</output>
			</div>
		)
	return (
		<div className="flex h-full items-center justify-center">
			<div className="max-w-md px-4 text-center">
				<p
					role={state.status === "error" ? "alert" : "status"}
					className="text-sm font-medium text-muted-foreground"
				>
					{state.status === "offline"
						? "Runtime disconnected"
						: state.status === "error"
							? "Could not open session"
							: "Session not found"}
				</p>
				<p className="mt-1 break-words text-xs text-muted-foreground/60">
					{state.status === "offline"
						? "Reconnect to the runtime to open this session. It will be checked again when the connection returns."
						: state.status === "error"
							? state.message
							: "The runtime could not find this session. It may have been deleted."}
				</p>
				{state.status !== "offline" && (
					<button
						type="button"
						onClick={onRetry}
						aria-label="Retry opening session"
						className="mt-3 rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
					>
						Try again
					</button>
				)}
			</div>
		</div>
	)
}
