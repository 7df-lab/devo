import { describe, expect, test } from "bun:test"
import { resolveSelectedProjectDirectory } from "./project-selection"
import type { SidebarProject } from "./types"

function project(name: string, directory: string, lastActiveAt: number): SidebarProject {
	return {
		id: name,
		slug: `${name}-slug`,
		name,
		directory,
		agentCount: 1,
		lastActiveAt,
		hasActiveAgent: false,
	}
}

describe("new chat project selection", () => {
	test("uses the route project even when root has newer sessions", () => {
		const projects = [
			project("/", "/", 300),
			project("devo", "/Users/tsiao/Desktop/devo", 200),
		]

		expect(resolveSelectedProjectDirectory(projects, "devo-slug", "")).toBe(
			"/Users/tsiao/Desktop/devo",
		)
	})

	test("keeps an explicit project choice when project activity changes", () => {
		const projects = [
			project("/", "/", 300),
			project("devo_feat_desktop", "/Users/tsiao/Desktop/devo_feat_desktop", 200),
		]

		expect(
				resolveSelectedProjectDirectory(
					projects,
					undefined,
					"/Users/tsiao/Desktop/devo_feat_desktop",
					{ preserveCurrentDirectory: true },
				),
			).toBe("/Users/tsiao/Desktop/devo_feat_desktop")
	})

	test("replaces an auto-selected parent directory when projects refresh", () => {
		const projects = [
			project("devo_feat_desktop", "/Users/tsiao/Desktop/devo_feat_desktop", 400),
			project("Desktop", "/Users/tsiao/Desktop", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "/Users/tsiao/Desktop", {
				preserveCurrentDirectory: false,
			}),
		).toBe("/Users/tsiao/Desktop/devo_feat_desktop")
	})

	test("does not default to filesystem root when a real project is available", () => {
		const projects = [
			project("/", "/", 300),
			project("devo_simplify_0623", "/Users/tsiao/Desktop/devo_simplify_0623", 0),
		]

		expect(resolveSelectedProjectDirectory(projects, undefined, "")).toBe(
			"/Users/tsiao/Desktop/devo_simplify_0623",
		)
	})
})
