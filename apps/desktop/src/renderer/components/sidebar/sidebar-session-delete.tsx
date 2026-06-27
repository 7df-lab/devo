import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
} from "@devo/ui/components/dialog"
import { AlertTriangleIcon, Loader2Icon, TrashIcon } from "lucide-react"
import type { Agent } from "../../lib/types"

export type DeleteSessionNavigationTarget =
	| {
			to: "/project/$projectSlug"
			params: { projectSlug: string }
	  }
	| { to: "/" }
	| null

export function deleteSessionNavigationTarget({
	deletedSessionId,
	currentSessionId,
	projectSlug,
}: {
	deletedSessionId: string
	currentSessionId: string | null | undefined
	projectSlug: string | undefined
}): DeleteSessionNavigationTarget {
	if (!currentSessionId || deletedSessionId !== currentSessionId) return null
	if (projectSlug) {
		return {
			to: "/project/$projectSlug",
			params: { projectSlug },
		}
	}
	return { to: "/" }
}

export function SessionDeleteDialog({
	agent,
	open,
	pending,
	error,
	onOpenChange,
	onConfirm,
}: {
	agent: Agent | null
	open: boolean
	pending: boolean
	error: string | null
	onOpenChange: (open: boolean) => void
	onConfirm: () => void
}) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !pending && onOpenChange(isOpen)}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<SessionDeleteDialogBody
					agent={agent}
					pending={pending}
					error={error}
					onCancel={() => onOpenChange(false)}
					onConfirm={onConfirm}
				/>
			</DialogContent>
		</Dialog>
	)
}

export function SessionDeleteDialogBody({
	agent,
	pending,
	error,
	onCancel,
	onConfirm,
}: {
	agent: Agent | null
	pending: boolean
	error: string | null
	onCancel: () => void
	onConfirm: () => void
}) {
	return (
		<>
			<div className="flex flex-col gap-2">
				<h2 className="flex items-center gap-2 text-lg font-semibold">
					<AlertTriangleIcon className="size-5 text-destructive" />
					Delete session
				</h2>
				<p className="text-sm text-muted-foreground">
					Delete{" "}
					<span className="font-medium text-foreground">{agent?.name || "this session"}</span>?
					This will remove the session history and cannot be undone.
				</p>
			</div>
			{error && (
				<div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
					{error}
				</div>
			)}
			<div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
				<Button variant="outline" disabled={pending} onClick={onCancel}>
					Cancel
				</Button>
				<Button variant="destructive" disabled={pending} onClick={onConfirm}>
					{pending ? (
						<Loader2Icon className="size-3.5 animate-spin" />
					) : (
						<TrashIcon className="size-3.5" />
					)}
					{pending ? "Deleting" : "Delete"}
				</Button>
			</div>
		</>
	)
}
