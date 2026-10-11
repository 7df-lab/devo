import { describe, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { defaultDevoSourcePath, runtimeBinaryName, stageRuntime } from "./prepare-runtime"
import { WINDOWS_SANDBOX_HELPERS } from "../../../scripts/runtime/backend"

describe("prepare-runtime helpers", () => {
	test("Windows staging requires and preserves both sandbox helpers", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-helper-staging-"))
		const source = join(root, "source")
		const desktopDir = join(root, "desktop")
		mkdirSync(source)
		writeFileSync(join(source, "devo.exe"), "backend")
		writeFileSync(join(source, "rg.exe"), "ripgrep")
		const options = { repoRoot: root, desktopDir, platform: "win32" as const, devoBin: join(source, "devo.exe"), rgBin: join(source, "rg.exe") }
		for (const helper of WINDOWS_SANDBOX_HELPERS) {
			expect(() => stageRuntime(options)).toThrow(`Windows sandbox helper not found: ${helper}`)
			expect(existsSync(join(desktopDir, "resources/runtime/bin"))).toBe(false)
			writeFileSync(join(source, helper), helper)
		}
		stageRuntime(options)
		expect(WINDOWS_SANDBOX_HELPERS.map(name => readFileSync(join(desktopDir, "resources/runtime/bin", name), "utf8"))).toEqual([...WINDOWS_SANDBOX_HELPERS])
	})

	test("a Windows bundle missing helpers cannot replace a staged Desktop runtime", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-helper-desktop-"))
		const bundleDir = join(root, "bundle")
		const desktopDir = join(root, "desktop")
		mkdirSync(join(bundleDir, "runtime"), { recursive: true })
		mkdirSync(join(desktopDir, "resources/runtime"), { recursive: true })
		writeFileSync(join(desktopDir, "resources/runtime/keep.txt"), "existing")
		writeFileSync(join(bundleDir, "runtime/manifest.json"), JSON.stringify({ schema: 1, target: "aarch64-pc-windows-msvc" }))
		expect(() => stageRuntime({ repoRoot: root, desktopDir, bundleDir, targetTriple: "aarch64-pc-windows-msvc" })).toThrow("Windows sandbox helper not found")
		expect(readFileSync(join(desktopDir, "resources/runtime/keep.txt"), "utf8")).toBe("existing")
		for (const name of WINDOWS_SANDBOX_HELPERS) writeFileSync(join(bundleDir, name), name)
		stageRuntime({ repoRoot: root, desktopDir, bundleDir, targetTriple: "aarch64-pc-windows-msvc" })
		expect(WINDOWS_SANDBOX_HELPERS.map(name => readFileSync(join(desktopDir, "resources/runtime", name), "utf8"))).toEqual([...WINDOWS_SANDBOX_HELPERS])
	})
	test("uses platform executable names", () => {
		expect({
			darwin: runtimeBinaryName("devo", "darwin"),
			linux: runtimeBinaryName("devo", "linux"),
			win32: runtimeBinaryName("devo", "win32"),
		}).toEqual({
			darwin: "devo",
			linux: "devo",
			win32: "devo.exe",
		})
	})

	test("resolves cargo release output by target triple", () => {
		expect(
			defaultDevoSourcePath({
				repoRoot: "/repo",
				targetTriple: "x86_64-apple-darwin",
				platform: "darwin",
			}),
		).toBe(join("/repo", "target", "x86_64-apple-darwin", "release", "devo"))
	})

	test("derives Windows executable names from target triples", () => {
		expect(
			defaultDevoSourcePath({
				repoRoot: "/repo",
				targetTriple: "x86_64-pc-windows-msvc",
				platform: "darwin",
			}),
		).toBe(join("/repo", "target", "x86_64-pc-windows-msvc", "release", "devo.exe"))
	})

	test("requires explicit ripgrep sidecar for cross-target staging", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-runtime-test-"))
		const repoRoot = join(root, "repo")
		const desktopDir = join(root, "desktop")
		const targetDir = join(repoRoot, "target", "aarch64-apple-darwin", "release")
		mkdirSync(targetDir, { recursive: true })
		mkdirSync(desktopDir, { recursive: true })
		writeFileSync(join(targetDir, "devo"), "")

		expect(() =>
			stageRuntime({
				desktopDir,
				repoRoot,
				targetTriple: "aarch64-apple-darwin",
				hostPlatform: "linux",
				hostArch: "x64",
			}),
		).toThrow("ripgrep sidecar for cross-target aarch64-apple-darwin must be passed")
		expect(existsSync(join(desktopDir, "resources", "runtime", "bin"))).toBe(false)
	})

	test("stages Devo and ripgrep sidecars into the desktop runtime directory", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-runtime-test-"))
		const desktopDir = join(root, "desktop")
		const sourceDir = join(root, "source")
		const releaseDir = join(root, "target", "release")
		const devoBin = join(sourceDir, "devo")
		const rgBin = join(sourceDir, "rg")
		mkdirSync(sourceDir, { recursive: true })
		mkdirSync(releaseDir, { recursive: true })
		writeFileSync(devoBin, "devo")
		writeFileSync(rgBin, "rg")

		stageRuntime({
			desktopDir,
			repoRoot: root,
			platform: "darwin",
			devoBin,
			rgBin,
		})

		expect({
			devo: readFileSync(join(desktopDir, "resources", "runtime", "bin", "devo"), "utf8"),
			rg: readFileSync(join(desktopDir, "resources", "runtime", "bin", "rg"), "utf8"),
		}).toEqual({
			devo: "devo",
			rg: "rg",
		})
	})

})
