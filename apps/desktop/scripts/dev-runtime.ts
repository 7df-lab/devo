import { copyFileSync, mkdtempSync, readdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"

export function devCargoBuild({
	repoRoot,
	env,
	platform,
}: {
	repoRoot: string
	env: NodeJS.ProcessEnv
	platform: NodeJS.Platform
}): { command: string[]; program: string | undefined } {
	const command = ["cargo", "build", "--manifest-path", join(repoRoot, "Cargo.toml"), "-p", "devo-cli"]
	if (platform !== "win32") return { command: [...command, "--bin", "devo"], program: undefined }

	// Older desktop/CLI instances may still lock target/debug/devo.exe. Build
	// separately, then run immutable copies so future builds remain writable.
	const targetRoot = resolve(repoRoot, env.CARGO_TARGET_DIR?.trim() || "target", "desktop-dev")
	return {
		command: [...command, "-p", "devo-windows-sandbox", "--bins", "--target-dir", targetRoot],
		program: join(targetRoot, "debug", "devo.exe"),
	}
}

export function stageWindowsDevRuntime(program: string, stagingRoot = tmpdir()): string {
	const sourceDir = dirname(program)
	const runtimeDir = mkdtempSync(join(stagingRoot, "devo-desktop-dev-"))
	const required = ["devo.exe", "devo-command-runner.exe", "devo-windows-sandbox-setup.exe"]
	try {
		for (const name of required) copyFileSync(join(sourceDir, name), join(runtimeDir, name))
		// Keep any dynamically linked runtime dependencies beside the executable.
		for (const name of readdirSync(sourceDir)) {
			if (name.toLowerCase().endsWith(".dll")) copyFileSync(join(sourceDir, name), join(runtimeDir, name))
		}
		return join(runtimeDir, "devo.exe")
	} catch (error) {
		rmSync(runtimeDir, { recursive: true, force: true })
		throw error
	}
}
