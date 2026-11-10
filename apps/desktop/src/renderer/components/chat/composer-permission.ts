/**
 * Session permission profiles for the composer footer picker.
 *
 * Labels match the TUI permission preset picker so Desktop and terminal
 * describe the same three modes.
 */

export type ComposerPermissionProfile = "default" | "autoReview" | "fullAccess"

/** The picker is currently hidden. Explicit server-side permission and sandbox
 *  configuration remains effective; flip to true to restore the picker. */
export const PERMISSION_PICKER_ENABLED = false

/** Visual fallback for an unconfigured new session. The server owns effective
 *  defaults; this value must not overwrite explicit project configuration. */
export const DEFAULT_COMPOSER_PERMISSION_PROFILE: ComposerPermissionProfile = "fullAccess"

export const COMPOSER_PERMISSION_PROFILES: readonly {
	id: ComposerPermissionProfile
	label: string
	description: string
}[] = [
	{
		id: "default",
		label: "Ask for approval",
		description: "You approve sensitive tools",
	},
	{
		id: "autoReview",
		label: "Approve for me",
		description: "An AI reviewer may approve low-risk tools",
	},
	{
		id: "fullAccess",
		label: "Full access",
		description: "No sandbox and no approval prompts",
	},
]

export function parseComposerPermissionProfile(
	value: string | undefined | null,
): ComposerPermissionProfile {
	switch (value) {
		case "default":
			return "default"
		case "autoReview":
		case "auto-review":
			return "autoReview"
		case "fullAccess":
		case "full-access":
			return "fullAccess"
		default:
			return DEFAULT_COMPOSER_PERMISSION_PROFILE
	}
}

export function composerPermissionLabel(profile: ComposerPermissionProfile): string {
	return (
		COMPOSER_PERMISSION_PROFILES.find((entry) => entry.id === profile)?.label ?? "Ask for approval"
	)
}

const pendingComposerPermissionBySession = new Map<string, ComposerPermissionProfile>()

/** Carries the new-chat picker choice across navigation before wire settings land. */
export function stashComposerPermissionForSession(
	sessionId: string,
	profile: ComposerPermissionProfile,
): void {
	pendingComposerPermissionBySession.set(sessionId, profile)
}

export function takeComposerPermissionForSession(
	sessionId: string,
): ComposerPermissionProfile | undefined {
	const profile = pendingComposerPermissionBySession.get(sessionId)
	if (profile !== undefined) {
		pendingComposerPermissionBySession.delete(sessionId)
	}
	return profile
}
