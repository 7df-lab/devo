import { Button } from "@devo/ui/components/button"
import { type ErrorComponentProps, useRouter } from "@tanstack/react-router"
import { AlertTriangleIcon, ChevronDownIcon, RefreshCwIcon } from "lucide-react"
import { useState } from "react"

export function ErrorPage({ error, reset }: ErrorComponentProps) {
	const router = useRouter()
	const [showDetails, setShowDetails] = useState(false)

	const message = error instanceof Error ? error.message : "An unexpected error occurred"
	const stack = error instanceof Error ? error.stack : undefined

	return (
		<div className="flex h-full items-center justify-center p-6">
			<div className="w-full max-w-md space-y-6">
				{/* Icon */}
				<div className="flex justify-center">
					<div className="flex size-14 items-center justify-center rounded-full border border-destructive/20 bg-destructive/10">
						<AlertTriangleIcon className="size-7 text-destructive" />
					</div>
				</div>

				{/* Title + message */}
				<div className="text-center">
					<h1 className="text-lg font-semibold text-foreground">Something went wrong</h1>
					<p className="mt-2 text-sm leading-relaxed text-muted-foreground">{message}</p>
				</div>

				{/* Actions */}
				<div className="flex items-center justify-center gap-3">
					<Button
						variant="outline"
						size="sm"
						onClick={() => {
							reset()
							router.invalidate()
						}}
					>
						<RefreshCwIcon />
						Try again
					</Button>
					<Button variant="ghost" size="sm" onClick={() => router.navigate({ to: "/" })}>
						Go home
					</Button>
				</div>

				{/* Expandable stack trace */}
				{stack && (
					<div className="space-y-2">
						<button
							type="button"
							className="mx-auto flex items-center gap-1 text-xs text-muted-foreground/60 transition-colors hover:text-muted-foreground"
							onClick={() => setShowDetails((prev) => !prev)}
						>
							<ChevronDownIcon
								className={`size-3 transition-transform ${showDetails ? "rotate-180" : ""}`}
							/>
							{showDetails ? "Hide" : "Show"} details
						</button>

						{showDetails && (
							<div className="rounded-lg border border-border bg-muted/50 p-3">
								<pre className="max-h-48 overflow-auto text-[11px] leading-relaxed text-muted-foreground">
									<code>{stack}</code>
								</pre>
							</div>
						)}
					</div>
				)}
			</div>
		</div>
	)
}
