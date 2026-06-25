import { app, BrowserWindow, shell } from "electron"
import type { AppUpdater, UpdateInfo } from "electron-updater"

/**
 * Auto-updater module.
 *
 * Uses electron-updater to check for new releases on GitHub Releases.
 * Downloads updates in the background and notifies the renderer via IPC
 * so it can show a non-intrusive "update available" banner.
 *
 * The electron-updater module is lazily imported to avoid loading ~500KB+
 * of code during startup (especially in dev where the updater is a no-op).
 *
 * On macOS, Squirrel.Mac requires the app to be code-signed for in-place
 * updates. When the app is unsigned (CI builds with CSC_IDENTITY_AUTO_DISCOVERY=false),
 * we detect this and fall back to opening the GitHub release page so the user
 * can download the new version manually. Windows and Linux are unaffected.
 */

let _autoUpdater: AppUpdater | null = null

async function getAutoUpdater(): Promise<AppUpdater> {
	if (!_autoUpdater) {
		// electron-updater is CJS — the destructuring workaround is required
		// for ESM compatibility. See electron-builder#7976.
		const electronUpdater = await import("electron-updater")
		_autoUpdater = electronUpdater.default.autoUpdater
	}
	return _autoUpdater
}

// ============================================================
// Signing detection
// ============================================================

/**
 * Detect whether the running macOS .app bundle is properly code-signed
 * (i.e. signed with a real Apple Developer ID, not ad-hoc or unsigned).
 * Returns true on non-macOS platforms since signing isn't required there.
 */
function detectCanAutoInstall(): boolean {
	if (process.platform !== "darwin") return true
	if (!app.isPackaged) return true

	try {
		const { execSync } = require("node:child_process")
		// codesign --verify exits 0 if valid signature, non-zero otherwise
		execSync(`codesign --verify --deep --strict "${app.getPath("exe")}"`, {
			encoding: "utf8",
			stdio: "pipe",
		})
		return true
	} catch {
		// Unsigned or ad-hoc signed — Squirrel.Mac will reject the install
		return false
	}
}

/** Whether the current build supports automatic in-place updates. */
let canAutoInstall = true

// ============================================================
// State
// ============================================================

/** Current update state, queryable by the renderer. */
export interface UpdateState {
	status: "idle" | "checking" | "available" | "downloading" | "ready" | "error"
	version?: string
	releaseNotes?: string
	progress?: {
		percent: number
		bytesPerSecond: number
		transferred: number
		total: number
	}
	error?: string
	/** Whether the app can auto-install updates (false on unsigned macOS builds). */
	canAutoInstall: boolean
}

let state: UpdateState = { status: "idle", canAutoInstall: true }
let checkInterval: ReturnType<typeof setInterval> | null = null

function getMainWindow(): BrowserWindow | null {
	return BrowserWindow.getAllWindows()[0] ?? null
}

function setState(next: Partial<UpdateState>): void {
	state = { ...state, ...next }
	getMainWindow()?.webContents.send("updater:state-changed", state)
}

// ============================================================
// GitHub release URL
// ============================================================

const GITHUB_REPO_URL = "https://github.com/tsiao/devo"

/** Build the GitHub release URL for a specific version tag. */
function getReleaseUrl(version?: string): string {
	if (version) {
		return `${GITHUB_REPO_URL}/releases/tag/v${version}`
	}
	return `${GITHUB_REPO_URL}/releases/latest`
}

// ============================================================
// Public API
// ============================================================

/**
 * Initialises the auto-updater. Call once after the main window is created.
 * In development (unpackaged), this is a no-op.
 */
export async function initAutoUpdater(): Promise<void> {
	if (!app.isPackaged) return

	canAutoInstall = detectCanAutoInstall()
	state = { ...state, canAutoInstall }

	console.log(
		`[auto-updater] platform=${process.platform}, canAutoInstall=${canAutoInstall}`,
	)

	const autoUpdater = await getAutoUpdater()

	// Logging
	autoUpdater.logger = console

	// Don't auto-download — let the user trigger it, or download silently
	// after they've been notified.
	autoUpdater.autoDownload = false

	// Install on quit by default (only effective when canAutoInstall is true)
	autoUpdater.autoInstallOnAppQuit = canAutoInstall

	// Skip code-signature verification on macOS. The CI builds are currently
	// unsigned (CSC_IDENTITY_AUTO_DISCOVERY=false) so the Squirrel/ShipIt
	// updater rejects the downloaded .app. Remove this once Apple Developer
	// code signing is configured in the release workflow.
	autoUpdater.forceDevUpdateConfig = true

	// ── Events ──────────────────────────────────────────────────

	autoUpdater.on("checking-for-update", () => {
		setState({ status: "checking" })
	})

	autoUpdater.on("update-available", (info: UpdateInfo) => {
		setState({
			status: "available",
			version: info.version,
			releaseNotes:
				typeof info.releaseNotes === "string"
					? info.releaseNotes
					: Array.isArray(info.releaseNotes)
						? info.releaseNotes.map((n) => n.note).join("\n")
						: undefined,
		})
	})

	autoUpdater.on("update-not-available", () => {
		setState({ status: "idle" })
	})

	autoUpdater.on("download-progress", (progress) => {
		setState({
			status: "downloading",
			progress: {
				percent: progress.percent,
				bytesPerSecond: progress.bytesPerSecond,
				transferred: progress.transferred,
				total: progress.total,
			},
		})
	})

	autoUpdater.on("update-downloaded", () => {
		setState({ status: "ready", progress: undefined })
	})

	autoUpdater.on("error", (err) => {
		console.error("[auto-updater] Error:", err.message)
		setState({ status: "error", error: err.message })
	})

	// ── Initial check (10s after launch) + periodic (every 4 hours) ──

	setTimeout(() => {
		autoUpdater.checkForUpdates().catch(() => {})
	}, 10_000)

	checkInterval = setInterval(
		() => {
			autoUpdater.checkForUpdates().catch(() => {})
		},
		4 * 60 * 60 * 1000,
	)
}

/** Returns the current update state (for IPC handler). */
export function getUpdateState(): UpdateState {
	return state
}

/** Manually triggers an update check. */
export async function checkForUpdates(): Promise<void> {
	if (!app.isPackaged) return
	const autoUpdater = await getAutoUpdater()
	await autoUpdater.checkForUpdates()
}

/** Starts downloading the available update. */
export async function downloadUpdate(): Promise<void> {
	const autoUpdater = await getAutoUpdater()
	await autoUpdater.downloadUpdate()
}

/**
 * Quits and installs the downloaded update.
 * Only works when canAutoInstall is true (signed macOS builds, Windows, Linux).
 */
export async function installUpdate(): Promise<void> {
	if (!canAutoInstall) {
		// Fallback: open release page instead of attempting Squirrel install
		await openReleasePage()
		return
	}
	const autoUpdater = await getAutoUpdater()
	autoUpdater.quitAndInstall(false, true)
}

/**
 * Opens the GitHub release page for the current update version in the
 * user's default browser. Used as a fallback on unsigned macOS builds
 * where Squirrel.Mac cannot perform in-place updates.
 */
export async function openReleasePage(): Promise<void> {
	const url = getReleaseUrl(state.version)
	await shell.openExternal(url)
}

/** Cleanup — call on app quit. */
export function stopAutoUpdater(): void {
	if (checkInterval) {
		clearInterval(checkInterval)
		checkInterval = null
	}
}
