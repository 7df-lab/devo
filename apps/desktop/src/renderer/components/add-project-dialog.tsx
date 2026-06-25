/**
 * Dialog for adding a project directory on a remote server.
 *
 * When connected to a remote server, shows a text input for typing
 * the remote directory path (since the native picker shows the local
 * filesystem which is irrelevant for a remote machine).
 *
 * For local servers, the caller should use `pickDirectory()` directly
 * instead of opening this dialog.
 */

import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogClose,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Label } from "@devo/ui/components/label"
import { useAtomValue } from "jotai"
import { FolderOpenIcon, Loader2Icon } from "lucide-react"
import { useCallback, useState } from "react"
import { activeServerConfigAtom } from "../atoms/connection"
import { loadProjectSessions } from "../services/connection-manager"

interface AddProjectDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	/** Called after a project directory is successfully added. */
	onAdded?: (directory: string) => void
}

/**
 * Dialog for entering a remote project directory path.
 * Only renders when connected to a non-local server.
 */
export function AddProjectDialog({ open, onOpenChange, onAdded }: AddProjectDialogProps) {
	const activeServer = useAtomValue(activeServerConfigAtom)

	const [remotePath, setRemotePath] = useState("")
	const [loading, setLoading] = useState(false)
	const [error, setError] = useState<string | null>(null)

	const handleOpenChange = useCallback(
		(nextOpen: boolean) => {
			if (nextOpen) {
				setRemotePath("")
				setError(null)
				setLoading(false)
			}
			onOpenChange(nextOpen)
		},
		[onOpenChange],
	)

	const handleAdd = useCallback(async () => {
		const trimmed = remotePath.trim()
		if (!trimmed) return

		setLoading(true)
		setError(null)
		try {
			await loadProjectSessions(trimmed)
			onAdded?.(trimmed)
			onOpenChange(false)
		} catch (err) {
			setError(
				err instanceof Error
					? err.message
					: "Failed to load project. Check that the path exists on the remote server.",
			)
		} finally {
			setLoading(false)
		}
	}, [remotePath, onAdded, onOpenChange])

	return (
		<Dialog open={open} onOpenChange={handleOpenChange}>
			<DialogContent className="sm:max-w-md">
				<DialogHeader>
					<DialogTitle>Add Remote Project</DialogTitle>
					<DialogDescription>
						Enter the absolute path to a project directory on{" "}
						<span className="font-medium text-foreground">{activeServer.name}</span>
					</DialogDescription>
				</DialogHeader>

				<div className="space-y-4 py-4">
					<div className="space-y-2">
						<Label htmlFor="remote-project-path">Directory Path</Label>
						<div className="relative">
							<FolderOpenIcon
								aria-hidden="true"
								className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
							/>
							<Input
								id="remote-project-path"
								placeholder="/home/user/projects/my-app"
								value={remotePath}
								onChange={(e) => {
									setRemotePath(e.target.value)
									setError(null)
								}}
								onKeyDown={(e) => {
									if (e.key === "Enter" && remotePath.trim()) {
										handleAdd()
									}
								}}
								className="pl-9"
								autoFocus
							/>
						</div>
						<p className="text-xs text-muted-foreground">
							The path must exist on the remote server. Sessions and project data will be loaded
							from this directory.
						</p>
					</div>

					{error && (
						<div className="rounded-md border border-destructive/20 bg-destructive/10 px-3 py-2 text-sm text-destructive">
							{error}
						</div>
					)}
				</div>

				<DialogFooter>
					<DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
					<Button disabled={!remotePath.trim() || loading} onClick={handleAdd}>
						{loading && <Loader2Icon aria-hidden="true" className="size-3.5 animate-spin" />}
						Add Project
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
