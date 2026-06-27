import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { SidebarProject } from "../../lib/types"
import {
	FolderRemoveDialogBody,
	MissingFolderDialogBody,
} from "./sidebar-folder-dialogs"

const project: SidebarProject = {
	id: "project-1",
	slug: "devo-1",
	name: "devo",
	directory: "/Users/tester/devo",
	agentCount: 0,
	lastActiveAt: 0,
	hasActiveAgent: false,
}

describe("sidebar folder dialogs", () => {
	test("explains remove only affects Devo Desktop and not disk", () => {
		const markup = renderToStaticMarkup(
			<FolderRemoveDialogBody
				project={project}
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			title: markup.includes("Remove folder from Devo Desktop"),
			desktopOnly: markup.includes("only removes it from the Desktop sidebar"),
			notDisk: markup.includes("does not delete anything from disk"),
		}).toEqual({
			title: true,
			desktopOnly: true,
			notDisk: true,
		})
	})

	test("asks whether to remove missing folders from Devo Desktop", () => {
		const markup = renderToStaticMarkup(
			<MissingFolderDialogBody
				project={project}
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirmRemove={() => {}}
			/>,
		)

		expect({
			title: markup.includes("Folder no longer exists"),
			removeQuestion: markup.includes("Remove it from Devo Desktop"),
		}).toEqual({
			title: true,
			removeQuestion: true,
		})
	})
})
