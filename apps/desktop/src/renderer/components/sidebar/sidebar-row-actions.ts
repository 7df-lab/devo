export type SidebarRowActionVariant = "default" | "destructive"
export type SessionRowActionId = "rename" | "fork" | "delete"
export type ProjectRowActionId =
	| "pin"
	| "reveal"
	| "create-worktree"
	| "rename"
	| "archive-chats"
	| "remove"

export interface SidebarRowAction<TId extends string = string> {
	id: TId
	label: string
	variant: SidebarRowActionVariant
	disabled?: boolean
}

export interface BuildSessionRowActionsArgs {
	canRename: boolean
	canFork: boolean
	canDelete: boolean
}

export interface BuildProjectRowActionsArgs {
	canRevealInFinder: boolean
}

export function buildSessionRowActions({
	canRename,
	canFork,
	canDelete,
}: BuildSessionRowActionsArgs): SidebarRowAction<SessionRowActionId>[] {
	const actions: SidebarRowAction<SessionRowActionId>[] = []
	if (canRename) actions.push({ id: "rename", label: "Rename", variant: "default" })
	if (canFork) actions.push({ id: "fork", label: "Fork", variant: "default" })
	if (canDelete) actions.push({ id: "delete", label: "Delete", variant: "destructive" })
	return actions
}

export function buildProjectRowActions({
	canRevealInFinder,
}: BuildProjectRowActionsArgs): SidebarRowAction<ProjectRowActionId>[] {
	return [
		{ id: "pin", label: "Pin project", variant: "default", disabled: true },
		{
			id: "reveal",
			label: "Reveal in Finder",
			variant: "default",
			disabled: !canRevealInFinder,
		},
		{
			id: "create-worktree",
			label: "Create permanent worktree",
			variant: "default",
			disabled: true,
		},
		{ id: "rename", label: "Rename project", variant: "default", disabled: true },
		{ id: "archive-chats", label: "Archive chats", variant: "default", disabled: true },
		{ id: "remove", label: "Remove", variant: "default", disabled: false },
	]
}
