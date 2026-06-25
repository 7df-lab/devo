/**
 * Shared desktop settings defaults.
 *
 * Used by both the Electron main process and the renderer. Keep this module
 * free of Electron or React imports so it can be bundled in either context.
 */

import type { AppSettings, AppearanceSettings, NotificationSettings, OpenInSettings } from "../preload/api"
import { DEFAULT_SERVER_SETTINGS } from "./server-config"

export const DEFAULT_NOTIFICATION_SETTINGS: NotificationSettings = {
	completionMode: "unfocused",
	permissions: true,
	questions: true,
	errors: true,
	dockBadge: true,
}

export const DEFAULT_APPEARANCE_SETTINGS: AppearanceSettings = {
	colorScheme: "dark",
	themeId: "default",
	displayMode: "default",
	rendererPreferencesMigrated: false,
}

export const DEFAULT_OPEN_IN_SETTINGS: OpenInSettings = {
	preferredTargetId: null,
}

export const DEFAULT_APP_SETTINGS: AppSettings = {
	notifications: DEFAULT_NOTIFICATION_SETTINGS,
	opaqueWindows: false,
	appearance: DEFAULT_APPEARANCE_SETTINGS,
	openIn: DEFAULT_OPEN_IN_SETTINGS,
	servers: DEFAULT_SERVER_SETTINGS,
}
