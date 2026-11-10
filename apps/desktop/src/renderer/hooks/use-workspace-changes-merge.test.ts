import { describe, expect, test } from "bun:test"
import type { WorkspaceChangeView } from "@devo-ai/sdk/v2/client"
import { mergePathFullIntoView } from "./use-workspace-changes"

function baseView(overrides: Partial<WorkspaceChangeView> = {}): WorkspaceChangeView {
	return {
		scope: "uncommitted",
		status: "ready",
		workspaceRoot: "/repo",
		coverage: "git_visible",
		attribution: "git_working_tree",
		changeSetStatus: "accumulating",
		files: [
			{
				path: "src/a.ts",
				status: "modified",
				additions: 1n,
				deletions: 1n,
				binary: false,
				diffTruncated: false,
			},
		],
		stats: { filesChanged: 1n, additions: 1n, deletions: 1n },
		warnings: [],
		generatedAt: "2026-06-26T00:00:00Z",
		...overrides,
	} as WorkspaceChangeView
}

describe("mergePathFullIntoView", () => {
	test("merges oldText/newText from path-scoped Full", () => {
		const base = baseView()
		const patch = baseView({
			files: [
				{
					path: "src/a.ts",
					status: "modified",
					additions: 1n,
					deletions: 1n,
					binary: false,
					diffTruncated: false,
					oldText: "old body\n",
					newText: "new body\n",
				},
			],
			unifiedDiff: [
				"diff --git a/src/a.ts b/src/a.ts",
				"--- a/src/a.ts",
				"+++ b/src/a.ts",
				"@@ -1 +1 @@",
				"-old body",
				"+new body",
			].join("\n"),
		})

		const merged = mergePathFullIntoView(base, patch)
		expect(merged.files[0]).toEqual(
			expect.objectContaining({
				oldText: "old body\n",
				newText: "new body\n",
			}),
		)
		expect(merged.unifiedDiff).toContain("+new body")
	})
})
