import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { homedir, tmpdir } from "node:os"
import { delimiter, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { bundleRuntime } from "../../../scripts/runtime/bundle"
import { WINDOWS_SANDBOX_HELPERS } from "../../../scripts/runtime/backend"

interface DefaultSourcePathOptions {
	repoRoot: string
	targetTriple?: string
	platform?: NodeJS.Platform
}

interface StageRuntimeOptions extends DefaultSourcePathOptions {
	bundleDir?: string
	devoBin?: string
	desktopDir: string
	hostArch?: NodeJS.Architecture
	hostPlatform?: NodeJS.Platform
	rgBin?: string
}

const scriptDir = dirname(fileURLToPath(import.meta.url))
const desktopDir = join(scriptDir, "..")
const repoRoot = join(desktopDir, "..", "..")

export function runtimeBinaryName(name: string, platform: NodeJS.Platform): string {
	return platform === "win32" ? `${name}.exe` : name
}

export function platformForTargetTriple(
	targetTriple: string | undefined,
	fallback: NodeJS.Platform,
): NodeJS.Platform {
	if (!targetTriple) return fallback
	if (targetTriple.includes("pc-windows-msvc")) return "win32"
	if (targetTriple.includes("apple-darwin")) return "darwin"
	if (targetTriple.includes("unknown-linux")) return "linux"
	return fallback
}

export function defaultDevoSourcePath({
	repoRoot,
	targetTriple,
	platform = process.platform,
}: DefaultSourcePathOptions): string {
	const targetParts = targetTriple ? ["target", targetTriple, "release"] : ["target", "release"]
	return join(repoRoot, ...targetParts, runtimeBinaryName("devo", platformForTargetTriple(targetTriple, platform)))
}

export function stageRuntime(options: StageRuntimeOptions): void {
	if (options.bundleDir) {
		const manifest = JSON.parse(readFileSync(join(options.bundleDir, "runtime/manifest.json"), "utf8"))
		if (manifest.schema !== 1 || manifest.target !== options.targetTriple) {
			throw new Error("Desktop runtime bundle target/schema mismatch")
		}
		if (manifest.target.includes("windows")) {
			for (const name of WINDOWS_SANDBOX_HELPERS) {
				if (!existsSync(join(options.bundleDir, name))) throw new Error(`Windows sandbox helper not found: ${name}`)
			}
		}
		const destination = join(options.desktopDir, "resources", "runtime")
		rmSync(destination, { recursive: true, force: true })
		cpSync(options.bundleDir, destination, { recursive: true, dereference: true })
		console.log(`Prepared complete Desktop runtime: ${destination}`)
		return
	}
	const devoSource = options.devoBin ?? defaultDevoSourcePath(options)
	const targetPlatform = platformForTargetTriple(options.targetTriple, options.platform ?? process.platform)
	const rgOverride = options.rgBin ?? optionalPath(process.env.DEVO_DESKTOP_RUNTIME_RG_BIN)

	if (
		!rgOverride &&
		options.targetTriple &&
		!targetMatchesHost({
			targetTriple: options.targetTriple,
			platform: options.hostPlatform ?? process.platform,
			arch: options.hostArch ?? process.arch,
		})
	) {
		throw new Error(
			`ripgrep sidecar for cross-target ${options.targetTriple} must be passed with --rg-bin or DEVO_DESKTOP_RUNTIME_RG_BIN`,
		)
	}

	const rgSource =
		rgOverride ??
		findExecutable(runtimeBinaryName("rg", targetPlatform), [
			join(homedir(), ".devo", "bin"),
			...(process.env.PATH ?? "").split(delimiter),
		])

	if (!existsSync(devoSource)) {
		throw new Error(`Devo runtime binary not found at ${devoSource}`)
	}
	if (!rgSource || !existsSync(rgSource)) {
		throw new Error("ripgrep sidecar not found. Install rg or pass --rg-bin <path>.")
	}
	const helpers = targetPlatform === "win32" ? WINDOWS_SANDBOX_HELPERS : []
	for (const name of helpers) {
		if (!existsSync(join(dirname(devoSource), name))) throw new Error(`Windows sandbox helper not found: ${name}`)
	}

	const runtimeBinDir = join(options.desktopDir, "resources", "runtime", "bin")
	rmSync(runtimeBinDir, { recursive: true, force: true })
	mkdirSync(runtimeBinDir, { recursive: true })

	const devoDest = join(runtimeBinDir, runtimeBinaryName("devo", targetPlatform))
	const rgDest = join(runtimeBinDir, runtimeBinaryName("rg", targetPlatform))
	copyExecutable(devoSource, devoDest, targetPlatform)
	copyExecutable(rgSource, rgDest, targetPlatform)
	for (const name of helpers) copyExecutable(join(dirname(devoSource), name), join(runtimeBinDir, name), targetPlatform)

	console.log(`Prepared Desktop runtime: ${devoDest}`)
	console.log(`Prepared ripgrep sidecar: ${rgDest}`)
}

function targetMatchesHost({
	targetTriple,
	platform,
	arch,
}: {
	targetTriple: string
	platform: NodeJS.Platform
	arch: NodeJS.Architecture
}): boolean {
	const targetPlatform = platformForTargetTriple(targetTriple, platform)
	return targetPlatform === platform && targetTriple.startsWith(`${targetArchName(arch)}-`)
}

function targetArchName(arch: NodeJS.Architecture): string {
	if (arch === "x64") return "x86_64"
	if (arch === "arm64") return "aarch64"
	return arch
}

function copyExecutable(source: string, dest: string, platform: NodeJS.Platform): void {
	copyFileSync(source, dest)
	if (platform !== "win32") chmodSync(dest, 0o755)
}

function findExecutable(name: string, dirs: string[]): string | null {
	for (const dir of dirs) {
		if (!dir) continue
		const candidate = join(dir, name)
		if (existsSync(candidate)) return candidate
	}
	return null
}

function argValue(name: string): string | undefined {
	const index = process.argv.indexOf(name)
	return index >= 0 ? process.argv[index + 1] : undefined
}

function optionalPath(value: string | undefined): string | undefined {
	const trimmed = value?.trim()
	return trimmed ? trimmed : undefined
}

if (import.meta.main) {
	const target = argValue("--target") ?? `${targetArchName(process.arch)}-${process.platform === "win32" ? "pc-windows-msvc" : process.platform === "darwin" ? "apple-darwin" : "unknown-linux-musl"}`
	const options: StageRuntimeOptions = {
		desktopDir,
		repoRoot,
		targetTriple: target,
		platform: process.platform,
		devoBin: argValue("--devo-bin") ?? optionalPath(process.env.DEVO_DESKTOP_RUNTIME_DEVO_BIN) ?? defaultDevoSourcePath({ repoRoot, targetTriple: argValue("--target") }),
		rgBin: argValue("--rg-bin"),
	}
	const bundleDir = argValue("--bundle-dir")
	if (bundleDir) {
		stageRuntime({ ...options, bundleDir })
	} else {
		const work = mkdtempSync(join(tmpdir(), "devo-desktop-runtime-"))
		try {
			stageRuntime({ ...options, desktopDir: work })
			const binaryDir = join(work, "resources/runtime/bin")
			const output = join(work, "bundle")
			await bundleRuntime(target, output, join(binaryDir, runtimeBinaryName("rg", platformForTargetTriple(target, process.platform))), binaryDir)
			stageRuntime({ ...options, bundleDir: output })
		} finally {
			rmSync(work, { recursive: true, force: true })
		}
	}
}
