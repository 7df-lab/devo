import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { DEFAULT_SERVER_SETTINGS } from "../shared/server-config"

let userDataDir = ""

mock.module("electron", () => ({
	app: {
		getPath: (name: string) => {
			if (name !== "userData") {
				throw new Error(`Unexpected app path request: ${name}`)
			}
			return userDataDir
		},
	},
}))

function expectedSettings() {
	return {
		notifications: {
			completionMode: "unfocused",
			permissions: true,
			questions: true,
			errors: true,
			dockBadge: true,
		},
		opaqueWindows: false,
		appearance: {
			colorScheme: "dark",
			themeId: "default",
			displayMode: "default",
			rendererPreferencesMigrated: false,
		},
		openIn: {
			preferredTargetId: null,
		},
		servers: DEFAULT_SERVER_SETTINGS,
	}
}

async function loadSettingsStore(name: string) {
	return await import(`./settings-store?case=${name}-${Date.now()}`)
}

describe("settings-store", () => {
	beforeEach(() => {
		userDataDir = mkdtempSync(join(tmpdir(), "devo-settings-store-"))
	})

	afterEach(() => {
		rmSync(userDataDir, { recursive: true, force: true })
	})

	test("loads full desktop settings defaults on a fresh profile", async () => {
		const store = await loadSettingsStore("defaults")

		store.initSettingsStore()

		expect(store.getSettings()).toEqual(expectedSettings())
	})

	test("deep-merges new desktop settings and persists them across reloads", async () => {
		const store = await loadSettingsStore("persist")
		store.initSettingsStore()

		const updated = store.updateSettings({
			appearance: {
				colorScheme: "system",
			},
			openIn: {
				preferredTargetId: "cursor",
			},
		})

		expect(updated).toEqual({
			...expectedSettings(),
			appearance: {
				colorScheme: "system",
				themeId: "default",
				displayMode: "default",
				rendererPreferencesMigrated: false,
			},
			openIn: {
				preferredTargetId: "cursor",
			},
		})

		const persisted = JSON.parse(readFileSync(join(userDataDir, "settings.json"), "utf-8"))
		expect(persisted).toEqual(updated)

		const reloaded = await loadSettingsStore("reload")
		reloaded.initSettingsStore()

		expect(reloaded.getSettings()).toEqual(updated)
	})
})
