import { afterEach, beforeEach, expect, mock, setSystemTime, test } from "bun:test"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { basename, join } from "node:path"

let root: string
mock.module("../../src/utils/paths", () => ({
	ocBackupsDir: () => join(root, "backups"),
}))
const { createBackup, listBackups, restore } = await import("../../src/backup")

beforeEach(async () => {
	root = await mkdtemp(join(tmpdir(), "devo-backup-collisions-"))
	setSystemTime(new Date("2026-11-11T12:00:00.000Z"))
})

afterEach(async () => {
	setSystemTime()
	await rm(root, { recursive: true, force: true })
})

test("repeated backups in one instant retain independent restorable snapshots", async () => {
	const source = join(root, "config.json")
	await writeFile(source, "original")
	const first = await createBackup([source], "First")
	await writeFile(source, "updated")
	const second = await createBackup([source], "Second")
	expect(first).not.toBe(second)
	expect((await listBackups()).map((backup) => backup.manifest.description).sort()).toEqual([
		"First",
		"Second",
	])
	for (const [backup, content] of [
		[first, "original"],
		[second, "updated"],
	] as const) {
		expect(await restore(basename(backup!))).toEqual({
			restored: [source],
			removed: [],
			errors: [],
		})
		expect(await readFile(source, "utf8")).toBe(content)
	}
	await writeFile(source, "changed again")
	expect(await restore()).toEqual({ restored: [source], removed: [], errors: [] })
	expect(await readFile(source, "utf8")).toBe("updated")
})

test("concurrent backups cannot overwrite each other's files or manifests", async () => {
	const sources = await Promise.all(
		Array.from({ length: 8 }, async (_, index) => {
			const source = join(root, `source-${index}.json`)
			await writeFile(source, `content-${index}`)
			return source
		}),
	)
	await mkdir(join(root, "backups"), { recursive: true })
	const backups = await Promise.all(
		sources.map((source, index) => createBackup([source], `Backup ${index}`)),
	)
	expect(new Set(backups).size).toBe(sources.length)
	for (const [index, backup] of backups.entries()) {
		const manifest = JSON.parse(await readFile(join(backup!, "manifest.json"), "utf8"))
		expect(manifest.description).toBe(`Backup ${index}`)
		expect(manifest.files).toEqual([
			{
				originalPath: sources[index],
				backupFilename: "0001.dat",
				existedBefore: true,
			},
		])
		expect(await readFile(join(backup!, "files", "0001.dat"), "utf8")).toBe(`content-${index}`)
	}
})
