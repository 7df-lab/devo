import { afterEach, beforeEach, expect, mock, test } from "bun:test"
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { CanonicalConversionResult } from "../../src/types/canonical"

let root: string
mock.module("../../src/utils/paths", () => ({
	ocBackupsDir: () => join(root, "backups"),
	ocGlobalConfigPath: () => join(root, "config.json"),
}))
const { createBackup, listBackups } = await import("../../src/backup")
const { universalWrite } = await import("../../src/writer/universal")

beforeEach(async () => {
	root = await mkdtemp(join(tmpdir(), "devo-backup-read-errors-"))
})
afterEach(async () => {
	await rm(root, { recursive: true, force: true })
})

test.skipIf(process.platform === "win32" || process.getuid?.() === 0)(
	"an unreadable but writable configuration aborts import before any target changes",
	async () => {
		const target = join(root, "config.json")
		const extra = join(root, "rule.md")
		const original = '{"existing":"must survive"}'
		await writeFile(target, original)
		await writeFile(extra, "original rule")
		await chmod(target, 0o200)
		try {
			const conversion: CanonicalConversionResult = {
				sourceFormat: "claude-code",
				targetFormat: "devo",
				globalConfig: { incoming: "new" },
				projectConfigs: new Map(),
				agents: new Map(),
				commands: new Map(),
				rules: new Map([[extra, "new rule"]]),
				extraFiles: new Map(),
				report: { converted: [], skipped: [], warnings: [], manualActions: [], errors: [] },
			}
			await expect(
				universalWrite(conversion, { backup: true, mergeStrategy: "preserve-existing", force: true }),
			).rejects.toMatchObject({ code: "EACCES" })
		} finally {
			await chmod(target, 0o600)
		}
		expect(await readFile(target, "utf8")).toBe(original)
		expect(await readFile(extra, "utf8")).toBe("original rule")
		expect(await listBackups()).toEqual([])
	},
)

test("a missing target is still recorded as a new file", async () => {
	const target = join(root, "new.json")
	const backup = await createBackup([target])
	const manifest = JSON.parse(await readFile(join(backup!, "manifest.json"), "utf8"))
	expect(manifest.files).toEqual([
		{ originalPath: target, backupFilename: "0001.dat", existedBefore: false },
	])
})
