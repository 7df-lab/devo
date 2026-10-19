import { rmSync } from "node:fs"
import { createRequire } from "node:module"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { devCargoBuild, stageWindowsDevRuntime } from "./dev-runtime"

const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const plan = devCargoBuild({ repoRoot: resolve(desktopDir, "../.."), env: process.env, platform: process.platform })
const build = Bun.spawn(plan.command, { cwd: desktopDir, stdin: "inherit", stdout: "inherit", stderr: "inherit" })
const buildExit = await build.exited
if (buildExit !== 0) process.exit(buildExit)
if (process.argv.includes("--build-only")) process.exit(0)

const override = process.env.DEVO_DESKTOP_DEVO_BIN?.trim()
const stagedProgram = !override && plan.program ? stageWindowsDevRuntime(plan.program) : undefined
try {
	if (stagedProgram) console.log(`Desktop development runtime: ${stagedProgram}`)
	const require = createRequire(import.meta.url)
	const electronVite = resolve(dirname(require.resolve("electron-vite/package.json")), "bin/electron-vite.js")
	const electron = Bun.spawn(["node", electronVite, ...process.argv.slice(2)], {
		cwd: desktopDir,
		env: { ...process.env, ...(stagedProgram ? { DEVO_DESKTOP_DEVO_BIN: stagedProgram } : {}) },
		stdin: "inherit",
		stdout: "inherit",
		stderr: "inherit",
	})
	process.exitCode = await electron.exited
} finally {
	if (stagedProgram) {
		// A surviving child may retain a Windows file lock after abnormal shutdown.
		// Never delete another launch's directory or terminate its processes.
		try {
			rmSync(dirname(stagedProgram), { recursive: true, force: true })
		} catch (error) {
			console.warn(`Could not clean development runtime ${dirname(stagedProgram)}: ${error}`)
		}
	}
}
