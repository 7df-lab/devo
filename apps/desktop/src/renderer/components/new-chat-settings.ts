import {
	type ComposerPermissionProfile,
	stashComposerPermissionForSession,
} from "./chat/composer-permission"

type LaunchSettings = {
	sessionId: string
	mode: "build" | "plan"
	permissionProfile?: ComposerPermissionProfile
}

type LaunchSettingsClient = {
	session?: {
		updateSettings?: (settings: LaunchSettings) => Promise<unknown>
	}
}

/** Apply explicit composer choices without replacing the server's inherited policy. */
export async function persistNewChatLaunchSettings(
	client: LaunchSettingsClient | null | undefined,
	{ sessionId, mode, permissionProfile }: LaunchSettings,
): Promise<void> {
	if (!client?.session?.updateSettings) {
		if (permissionProfile !== undefined) {
			throw new Error("Cannot apply the selected permission profile: session settings are unavailable")
		}
		return
	}
	try {
		await client.session.updateSettings({
			sessionId,
			mode,
			...(permissionProfile !== undefined ? { permissionProfile } : {}),
		})
	} catch (err) {
		// Never launch under a potentially more permissive inherited policy.
		if (permissionProfile !== undefined) throw err
		console.error("Failed to persist launch session settings:", err)
	}
	if (permissionProfile !== undefined) {
		stashComposerPermissionForSession(sessionId, permissionProfile)
	}
}
