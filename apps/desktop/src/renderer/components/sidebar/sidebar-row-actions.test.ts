import { describe, expect, test } from "bun:test"
import { buildProjectRowActions, buildSessionRowActions } from "./sidebar-row-actions"

describe("sidebar row actions", () => {
	test("session actions expose rename and fork before the destructive delete action", () => {
		expect(
			buildSessionRowActions({
				canRename: true,
				canFork: true,
				canDelete: true,
			}),
		).toEqual([
			{ id: "rename", label: "Rename", variant: "default" },
			{ id: "fork", label: "Fork", variant: "default" },
			{ id: "delete", label: "Delete", variant: "destructive" },
		])
	})

	test("session actions omit unavailable callbacks", () => {
		expect(
			buildSessionRowActions({
				canRename: false,
				canFork: true,
				canDelete: false,
			}),
		).toEqual([{ id: "fork", label: "Fork", variant: "default" }])
	})

	test("project actions expose remove as a regular enabled project action", () => {
		expect(buildProjectRowActions({ canRevealInFinder: true })).toEqual([
			{ id: "pin", label: "Pin project", variant: "default", disabled: true },
			{ id: "reveal", label: "Reveal in Finder", variant: "default", disabled: false },
			{
				id: "create-worktree",
				label: "Create permanent worktree",
				variant: "default",
				disabled: true,
			},
			{ id: "rename", label: "Rename project", variant: "default", disabled: true },
			{ id: "archive-chats", label: "Archive chats", variant: "default", disabled: true },
			{ id: "remove", label: "Remove", variant: "default", disabled: false },
		])
	})
})
