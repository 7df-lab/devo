import { describe, expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { devCargoBuild, stageWindowsDevRuntime } from "./dev-runtime"

describe("desktop development runtime", () => {
	test("Windows builds the CLI and sandbox helpers away from running checkout binaries", () => {
		const repoRoot = join(tmpdir(), "repo")
		const targetRoot = join(repoRoot, "target", "desktop-dev")
		expect(devCargoBuild({ repoRoot, env: {}, platform: "win32" })).toEqual({
			command: [
				"cargo", "build", "--manifest-path", join(repoRoot, "Cargo.toml"), "-p", "devo-cli",
				"-p", "devo-windows-sandbox", "--bins", "--target-dir", targetRoot,
			],
			program: join(targetRoot, "debug", "devo.exe"),
		})
	})

	test("isolates the desktop output inside a custom Cargo target directory", () => {
		const repoRoot = join(tmpdir(), "repo")
		const targetRoot = join(tmpdir(), "custom-target", "desktop-dev")
		expect(
			devCargoBuild({ repoRoot, env: { CARGO_TARGET_DIR: join(tmpdir(), "custom-target") }, platform: "win32" }).program,
		).toBe(join(targetRoot, "debug", "devo.exe"))
	})

	test.each(["linux", "darwin"] as const)("retains normal Cargo output on %s", (platform) => {
		const repoRoot = join(tmpdir(), "repo")
		expect(devCargoBuild({ repoRoot, env: {}, platform })).toEqual({
			command: ["cargo", "build", "--manifest-path", join(repoRoot, "Cargo.toml"), "-p", "devo-cli", "--bin", "devo"],
			program: undefined,
		})
	})

	test("stages independent launch generations with matching helpers and DLLs", () => {
		const fixture = mkdtempSync(join(tmpdir(), "devo-dev-runtime-test-"))
		try {
			const source = join(fixture, "debug")
			mkdirSync(source)
			const files = ["devo.exe", "devo-command-runner.exe", "devo-windows-sandbox-setup.exe", "runtime.dll"]
			for (const name of files) writeFileSync(join(source, name), `original ${name}`)
			writeFileSync(join(source, "unrelated.pdb"), "do not stage")
			const first = stageWindowsDevRuntime(join(source, "devo.exe"), fixture)
			writeFileSync(join(source, "devo.exe"), "rebuilt")
			const second = stageWindowsDevRuntime(join(source, "devo.exe"), fixture)
			expect(first).not.toBe(second)
			expect(readFileSync(first, "utf8")).toBe("original devo.exe")
			expect(readFileSync(second, "utf8")).toBe("rebuilt")
			expect(files.map((name) => readFileSync(join(dirname(first), name), "utf8")))
				.toEqual(files.map((name) => `original ${name}`))
			expect(readdirSync(dirname(first)).sort()).toEqual([...files].sort())
		} finally {
			rmSync(fixture, { recursive: true, force: true })
		}
	})

	test("removes an incomplete generation when a required sandbox helper is missing", () => {
		const fixture = mkdtempSync(join(tmpdir(), "devo-dev-runtime-test-"))
		try {
			const source = join(fixture, "debug")
			mkdirSync(source)
			writeFileSync(join(source, "devo.exe"), "CLI without helpers")
			expect(() => stageWindowsDevRuntime(join(source, "devo.exe"), fixture)).toThrow()
			expect(readdirSync(fixture)).toEqual(["debug"])
		} finally {
			rmSync(fixture, { recursive: true, force: true })
		}
	})
})
