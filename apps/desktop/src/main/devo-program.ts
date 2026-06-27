import fs from "node:fs"
import path from "node:path"

export interface ResolveDevoProgramOptions {
	appPath: string
	env: NodeJS.ProcessEnv
	existsSync?: (path: string) => boolean
	isPackaged: boolean
	platform?: NodeJS.Platform
	resourcesPath?: string
}

const PATH_DEVO = "devo"
const OVERRIDE_ENV = "DEVO_DESKTOP_DEVO_BIN"

export function resolveDevoProgram({
	appPath,
	env,
	existsSync = fs.existsSync,
	isPackaged,
	platform = process.platform,
	resourcesPath,
}: ResolveDevoProgramOptions): string {
	const override = env[OVERRIDE_ENV]?.trim()
	if (override) return override

	if (isPackaged) {
		const runtimeRoot = resourcesPath ?? path.join(appPath, "..")
		const bundled = path.join(runtimeRoot, "runtime", "bin", devoExecutableName(platform))
		if (existsSync(bundled)) return bundled
		throw new Error(`Bundled Devo runtime not found at ${bundled}`)
	}

	const checkoutRoot = path.resolve(appPath, "../..")
	const candidates = [
		path.join(checkoutRoot, "target", "debug", "devo.exe"),
		path.join(checkoutRoot, "target", "debug", "devo"),
		path.join(checkoutRoot, "target", "release", "devo.exe"),
		path.join(checkoutRoot, "target", "release", "devo"),
	]

	return candidates.find(existsSync) ?? PATH_DEVO
}

function devoExecutableName(platform: NodeJS.Platform): string {
	return platform === "win32" ? "devo.exe" : "devo"
}
