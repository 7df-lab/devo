import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { app } from "electron"

// ESM equivalent for __dirname
const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

/**
 * Resolves the path to the CLI shell script bundled inside the app.
 *
 * macOS packaged:  Devo.app/Contents/Resources/bin/devo
 * Linux packaged:  <install-dir>/resources/bin/devo
 * Windows:         <install-dir>/resources/bin/devo.cmd
 * Dev mode:        <repo>/apps/desktop/resources/bin/devo
 */
function getCliSourcePath(): string {
	const scriptName = process.platform === "win32" ? "devo.cmd" : "devo"

	if (app.isPackaged) {
		// In packaged apps, extraResources go into the resources directory
		return path.join(process.resourcesPath, "bin", scriptName)
	}

	// Dev mode — point to the source file in the repo
	return path.join(__dirname, "../../resources/bin", scriptName)
}

/**
 * Returns the directory where the CLI symlink will be installed.
 *
 * macOS/Linux: /usr/local/bin
 * Windows:     %LOCALAPPDATA%\Devo\bin  (added to PATH by the user)
 */
function getCliInstallDir(): string {
	if (process.platform === "win32") {
		const localAppData =
			process.env.LOCALAPPDATA ?? path.join(app.getPath("home"), "AppData", "Local")
		return path.join(localAppData, "Devo", "bin")
	}
	return "/usr/local/bin"
}

/**
 * Returns the full path where the `devo` symlink/script will be placed.
 */
function getCliInstallPath(): string {
	const name = process.platform === "win32" ? "devo.cmd" : "devo"
	return path.join(getCliInstallDir(), name)
}

/**
 * Checks whether the CLI command is currently installed.
 */
export function isCliInstalled(): boolean {
	const installPath = getCliInstallPath()
	try {
		const stat = fs.lstatSync(installPath)
		if (stat.isSymbolicLink()) {
			const target = fs.readlinkSync(installPath)
			return target === getCliSourcePath()
		}
		return stat.isFile()
	} catch {
		return false
	}
}

/**
 * Installs the `devo` CLI command by symlinking it into /usr/local/bin
 * (or the platform equivalent).
 *
 * Returns an object with `success` and optional `error` message.
 */
export function installCli(): { success: boolean; error?: string } {
	const source = getCliSourcePath()
	const dest = getCliInstallPath()
	const destDir = getCliInstallDir()

	try {
		// Verify the source script exists
		if (!fs.existsSync(source)) {
			return { success: false, error: `CLI script not found at ${source}` }
		}

		// Ensure destination directory exists
		fs.mkdirSync(destDir, { recursive: true })

		// Remove existing symlink/file if present
		try {
			fs.unlinkSync(dest)
		} catch {
			// Doesn't exist — that's fine
		}

		if (process.platform === "win32") {
			// On Windows, copy the script instead of symlinking
			fs.copyFileSync(source, dest)
		} else {
			// On Unix, create a symlink
			fs.symlinkSync(source, dest)
		}

		return { success: true }
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err)

		// Common case: /usr/local/bin not writable without sudo
		if (message.includes("EACCES") || message.includes("permission")) {
			return {
				success: false,
				error: `Permission denied. Try running:\n  sudo ln -sf "${source}" "${dest}"`,
			}
		}

		return { success: false, error: message }
	}
}

/**
 * Uninstalls the `devo` CLI command by removing the symlink.
 */
export function uninstallCli(): { success: boolean; error?: string } {
	const dest = getCliInstallPath()

	try {
		if (!fs.existsSync(dest)) {
			return { success: true } // Already gone
		}

		fs.unlinkSync(dest)
		return { success: true }
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err)

		if (message.includes("EACCES") || message.includes("permission")) {
			return {
				success: false,
				error: `Permission denied. Try running:\n  sudo rm "${dest}"`,
			}
		}

		return { success: false, error: message }
	}
}
