import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import { describe, expect, test } from "bun:test"
import { type GitBranchInfo, listBranches } from "./git-service"

function branchSummary(state: GitBranchInfo["state"]): GitBranchInfo {
	return {
		state,
		current: "",
		detached: false,
		local: [],
		remote: [],
	}
}

const notGitBranchSummary = branchSummary("not_git")
const missingBranchSummary = branchSummary("missing")
const notDirectoryBranchSummary = branchSummary("not_directory")

describe("git service", () => {
	test("returns an explicit not-git state for non-git directories", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		try {
			await expect(listBranches(directory)).resolves.toEqual(notGitBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})

	test("returns an explicit missing state for missing directories", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		const missingDirectory = path.join(directory, "missing")
		try {
			await expect(listBranches(missingDirectory)).resolves.toEqual(missingBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})

	test("returns an explicit not-directory state for non-directory paths", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		const filePath = path.join(directory, "file.txt")
		try {
			await writeFile(filePath, "not a directory")
			await expect(listBranches(filePath)).resolves.toEqual(notDirectoryBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})
})
