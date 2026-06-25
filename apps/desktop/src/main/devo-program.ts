import fs from "node:fs"
import path from "node:path"

export interface ResolveDevoProgramOptions {
	appPath: string
	env: NodeJS.ProcessEnv
	existsSync?: (path: string) => boolean
	isPackaged: boolean
}

const PATH_DEVO = "devo"
const OVERRIDE_ENV = "DEVO_DESKTOP_DEVO_BIN"

export function resolveDevoProgram({
	appPath,
	env,
	existsSync = fs.existsSync,
	isPackaged,
}: ResolveDevoProgramOptions): string {
	const override = env[OVERRIDE_ENV]?.trim()
	if (override) return override

	if (isPackaged) return PATH_DEVO

	const checkoutRoot = path.resolve(appPath, "../..")
	const candidates = [
		path.join(checkoutRoot, "target", "debug", "devo.exe"),
		path.join(checkoutRoot, "target", "debug", "devo"),
		path.join(checkoutRoot, "target", "release", "devo.exe"),
		path.join(checkoutRoot, "target", "release", "devo"),
	]

	return candidates.find(existsSync) ?? PATH_DEVO
}
