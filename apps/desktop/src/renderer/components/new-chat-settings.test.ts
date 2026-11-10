import { describe, expect, mock, spyOn, test } from "bun:test"
import {
	COMPOSER_PERMISSION_PROFILES,
	takeComposerPermissionForSession,
} from "./chat/composer-permission"
import { persistNewChatLaunchSettings } from "./new-chat-settings"

describe("new-chat launch settings", () => {
	for (const mode of ["build", "plan"] as const) {
		test(`inherits project permissions in ${mode} mode without writing or stashing a fallback`, async () => {
			const sessionId = `inherited-${mode}`
			const updateSettings = mock(async (_settings: Parameters<typeof persistNewChatLaunchSettings>[1]) => {})
			await persistNewChatLaunchSettings({ session: { updateSettings } }, {
				sessionId,
				mode,
				permissionProfile: undefined,
			})
			expect(updateSettings.mock.calls).toEqual([[{ sessionId, mode }]])
			expect(takeComposerPermissionForSession(sessionId)).toBeUndefined()
		})

		for (const { id: permissionProfile } of COMPOSER_PERMISSION_PROFILES) {
			test(`persists and stashes explicit ${permissionProfile} in ${mode} mode`, async () => {
				const sessionId = `${mode}-${permissionProfile}`
				const updateSettings = mock(async (_settings: Parameters<typeof persistNewChatLaunchSettings>[1]) => {})
				const settings = { sessionId, mode, permissionProfile }
				await persistNewChatLaunchSettings({ session: { updateSettings } }, settings)
				expect(updateSettings.mock.calls).toEqual([[settings]])
				expect([
					takeComposerPermissionForSession(sessionId),
					takeComposerPermissionForSession(sessionId),
				]).toEqual([permissionProfile, undefined])
			})
		}
	}

	for (const { id: permissionProfile } of COMPOSER_PERMISSION_PROFILES) {
		test(`rejects ${permissionProfile} without an available settings writer`, async () => {
			for (const client of [null, undefined, {}, { session: {} }]) {
				await expect(persistNewChatLaunchSettings(client, {
					sessionId: "no-client",
					mode: "build",
					permissionProfile,
				})).rejects.toThrow("session settings are unavailable")
				expect(takeComposerPermissionForSession("no-client")).toBeUndefined()
			}
		})

		test(`rejects a failed ${permissionProfile} write without stashing the unconfirmed choice`, async () => {
			const error = new Error("Settings unavailable")
			const updateSettings = mock(async () => { throw error })
			await expect(persistNewChatLaunchSettings({ session: { updateSettings } }, {
				sessionId: "failed-write",
				mode: "build",
				permissionProfile,
			})).rejects.toThrow(error)
			expect(takeComposerPermissionForSession("failed-write")).toBeUndefined()
		})
	}

	test("does not resolve or stash the explicit choice before the write is acknowledged", async () => {
		let acknowledge!: () => void
		const acknowledged = new Promise<void>((resolve) => { acknowledge = resolve })
		const updateSettings = mock(() => acknowledged)
		let finished = false
		const pending = persistNewChatLaunchSettings({ session: { updateSettings } }, {
			sessionId: "pending-write",
			mode: "build",
			permissionProfile: "default",
		}).then(() => { finished = true })
		await Promise.resolve()
		expect([finished, takeComposerPermissionForSession("pending-write")]).toEqual([false, undefined])
		acknowledge()
		await pending
		expect([finished, takeComposerPermissionForSession("pending-write")]).toEqual([true, "default"])
	})

	test("keeps inherited launches compatible with unavailable or failed settings writes", async () => {
		await persistNewChatLaunchSettings(null, { sessionId: "inherited-no-client", mode: "build" })
		const error = new Error("Settings unavailable")
		const updateSettings = mock(async () => { throw error })
		const logError = spyOn(console, "error").mockImplementation(() => {})
		try {
			await persistNewChatLaunchSettings({ session: { updateSettings } }, {
				sessionId: "inherited-failed-write",
				mode: "build",
			})
			expect(logError.mock.calls).toEqual([["Failed to persist launch session settings:", error]])
			expect(takeComposerPermissionForSession("inherited-failed-write")).toBeUndefined()
		} finally {
			logError.mockRestore()
		}
	})
})
