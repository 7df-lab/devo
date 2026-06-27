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

	test("clears an implicit root-page selection when projects refresh", () => {
		const projects = [
			project("devo_feat_desktop", "/Users/tsiao/Desktop/devo_feat_desktop", 400),
			project("Desktop", "/Users/tsiao/Desktop", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "/Users/tsiao/Desktop", {
				preserveCurrentDirectory: false,
			}),
		).toBe("")
	})

	test("does not choose a default project on the root route", () => {
		const projects = [
			project("/", "/", 300),
			project("devo_simplify_0623", "/Users/tsiao/Desktop/devo_simplify_0623", 0),
		]

		expect(resolveSelectedProjectDirectory(projects, undefined, "")).toBe("")
	})

	test("does not choose an unavailable-aware default project on the root route", () => {
		const projects = [
			project("old-worktree", "/Users/tsiao/Desktop/devo_missing", 400),
			project("devo", "/Users/tsiao/Desktop/devo", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "", {
				unavailableDirectories: new Set(["/Users/tsiao/Desktop/devo_missing"]),
			}),
		).toBe("")
	})

	test("keeps an explicitly routed unavailable project", () => {
		const projects = [
			project("old-worktree", "/Users/tsiao/Desktop/devo_missing", 400),
			project("devo", "/Users/tsiao/Desktop/devo", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, "old-worktree-slug", "", {
				unavailableDirectories: new Set(["/Users/tsiao/Desktop/devo_missing"]),
			}),
		).toBe("/Users/tsiao/Desktop/devo_missing")
	})
})
