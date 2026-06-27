import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import {
	createDesktopFolder,
	normalizeDesktopFolderDirectory,
	statDesktopFolders,
} from "./desktop-folders"

let rootDir = ""

describe("desktop folders", () => {
	beforeEach(async () => {
		rootDir = await mkdtemp(path.join(tmpdir(), "devo-desktop-folders-"))
	})

	afterEach(async () => {
		await rm(rootDir, { recursive: true, force: true })
	})

	test("normalizes trailing path separators without changing root paths", () => {
		expect(normalizeDesktopFolderDirectory(`${rootDir}${path.sep}`)).toBe(rootDir)
		expect(normalizeDesktopFolderDirectory(path.parse(rootDir).root)).toBe(path.parse(rootDir).root)
	})

	test("stats available, missing, and not-directory folder paths", async () => {
		const filePath = path.join(rootDir, "file.txt")
		const missingPath = path.join(rootDir, "missing")
		await writeFile(filePath, "content")

		await expect(statDesktopFolders([rootDir, missingPath, filePath])).resolves.toEqual([
			{ directory: rootDir, status: "available" },
			{ directory: missingPath, status: "missing" },
			{ directory: filePath, status: "not_directory" },
		])
	})

	test("creates a child folder under an existing parent directory", async () => {
		await expect(createDesktopFolder({ parentDirectory: rootDir, name: "new-project" })).resolves.toEqual({
			directory: path.join(rootDir, "new-project"),
			name: "new-project",
		})

		await expect(statDesktopFolders([path.join(rootDir, "new-project")])).resolves.toEqual([
			{ directory: path.join(rootDir, "new-project"), status: "available" },
		])
	})

	test("rejects invalid names, duplicate folders, files, and missing parents", async () => {
		const filePath = path.join(rootDir, "file.txt")
		await writeFile(filePath, "content")
		await createDesktopFolder({ parentDirectory: rootDir, name: "existing" })

		await expect(createDesktopFolder({ parentDirectory: rootDir, name: "" })).rejects.toThrow(
			"Folder name is required",
		)
		await expect(createDesktopFolder({ parentDirectory: rootDir, name: "nested/path" })).rejects.toThrow(
			"Folder name cannot contain path separators",
		)
		await expect(createDesktopFolder({ parentDirectory: rootDir, name: "existing" })).rejects.toThrow(
			"Folder already exists",
		)
		await expect(createDesktopFolder({ parentDirectory: filePath, name: "child" })).rejects.toThrow(
			"Parent path is not a directory",
		)
		await expect(
			createDesktopFolder({ parentDirectory: path.join(rootDir, "missing"), name: "child" }),
		).rejects.toThrow("Parent folder does not exist")
	})
})
