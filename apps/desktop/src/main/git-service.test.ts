import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import { describe, expect, test } from "bun:test"
import { listBranches } from "./git-service"

const emptyBranchSummary = {
	current: "",
	detached: false,
	local: [],
	remote: [],
}

describe("git service", () => {
	test("returns an empty branch summary for non-git directories", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		try {
			await expect(listBranches(directory)).resolves.toEqual(emptyBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})

	test("returns an empty branch summary for missing directories", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		const missingDirectory = path.join(directory, "missing")
		try {
			await expect(listBranches(missingDirectory)).resolves.toEqual(emptyBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})

	test("returns an empty branch summary for non-directory paths", async () => {
		const directory = await mkdtemp(path.join(tmpdir(), "devo-git-service-"))
		const filePath = path.join(directory, "file.txt")
		try {
			await writeFile(filePath, "not a directory")
			await expect(listBranches(filePath)).resolves.toEqual(emptyBranchSummary)
		} finally {
			await rm(directory, { recursive: true, force: true })
		}
	})
})
