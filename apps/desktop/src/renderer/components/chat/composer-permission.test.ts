import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import {
	COMPOSER_PERMISSION_PROFILES,
	composerPermissionLabel,
	parseComposerPermissionProfile,
	stashComposerPermissionForSession,
	takeComposerPermissionForSession,
} from "./composer-permission"

const pickerSource = readFileSync(new URL("./composer-permission-picker.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const newChatSource = readFileSync(new URL("../new-chat.tsx", import.meta.url), "utf8")

describe("composer permission profiles", () => {
	test("uses the TUI labels for the three presets", () => {
		expect(COMPOSER_PERMISSION_PROFILES.map((profile) => profile.label)).toEqual([
			"Ask for approval",
			"Approve for me",
			"Full access",
		])
		expect(parseComposerPermissionProfile("auto-review")).toBe("autoReview")
		expect(parseComposerPermissionProfile("fullAccess")).toBe("fullAccess")
		expect(parseComposerPermissionProfile("default")).toBe("default")
		expect(parseComposerPermissionProfile(undefined)).toBe("fullAccess")
		expect(composerPermissionLabel("autoReview")).toBe("Approve for me")
		expect(composerPermissionLabel("default")).toBe("Ask for approval")
	})

	test("places a compact footer picker on both composers without a search field", () => {
		expect({
			usesOptionMenu: pickerSource.includes("optionMenuContentClass"),
			sidebarIconStroke: pickerSource.includes("stroke-[1.5]"),
			noSearchField: !pickerSource.includes("placeholder") && !pickerSource.includes("SearchIcon"),
			chatViewWiresPicker: chatViewSource.includes("ComposerPermissionPicker"),
			newChatWiresPicker: newChatSource.includes("ComposerPermissionPicker"),
			defaultsToFullAccess:
				chatViewSource.includes("DEFAULT_COMPOSER_PERMISSION_PROFILE") &&
				newChatSource.includes("DEFAULT_COMPOSER_PERMISSION_PROFILE"),
			permissionSitsWithAttach:
				chatViewSource.includes("<AttachButton") &&
				chatViewSource.indexOf("<AttachButton") < chatViewSource.indexOf("<ComposerPermissionPicker"),
			modelSitsWithSend:
				chatViewSource.includes('className="ml-auto flex min-w-0 items-center gap-0.5"') &&
				newChatSource.includes('className="ml-auto flex min-w-0 items-center gap-0.5"'),
		}).toEqual({
			usesOptionMenu: true,
			sidebarIconStroke: true,
			noSearchField: true,
			chatViewWiresPicker: true,
			newChatWiresPicker: true,
			defaultsToFullAccess: true,
			permissionSitsWithAttach: true,
			modelSitsWithSend: true,
		})
	})
})

describe("new-chat permission inheritance", () => {
	test("keeps the visual fallback separate from an explicit permission selection", () => {
		expect(newChatSource).toContain(
			"useState<ComposerPermissionProfile | undefined>(undefined)",
		)
		expect(newChatSource).toContain(
			"value={permissionProfile ?? DEFAULT_COMPOSER_PERMISSION_PROFILE}",
		)
		expect(newChatSource).toContain("onChange={setPermissionProfile}")
	})

	test("uses shared settings persistence for local and worktree launches", () => {
		expect(newChatSource).toContain(
			"await persistNewChatLaunchSettings(getProjectClient(selectedDirectory), {",
		)
		expect(newChatSource).toContain(
			"await persistNewChatLaunchSettings(getProjectClient(sdkDirectory), {",
		)
	})

	test("both launch paths await confirmed settings before navigating or sending a prompt", () => {
		const localStart = newChatSource.indexOf("const launchLocal = useCallback(")
		const worktreeStart = newChatSource.indexOf("const launchWorktree = useCallback(")
		const handleStart = newChatSource.indexOf("const handleLaunch = useCallback(")
		for (const source of [
			newChatSource.slice(localStart, worktreeStart),
			newChatSource.slice(worktreeStart, handleStart),
		]) {
			const persist = source.indexOf("await persistNewChatLaunchSettings(")
			expect(persist).toBeGreaterThanOrEqual(0)
			expect(source.indexOf("navigateToSession(session.id)")).toBeGreaterThan(persist)
			expect(source.indexOf("await sendPrompt(")).toBeGreaterThan(persist)
		}
	})

	test("leaves inherited sessions unstashed and preserves every explicit profile once", () => {
		expect(takeComposerPermissionForSession("inherited-session")).toBeUndefined()
		for (const { id } of COMPOSER_PERMISSION_PROFILES) {
			const sessionId = `explicit-${id}`
			stashComposerPermissionForSession(sessionId, id)
			expect([
				takeComposerPermissionForSession(sessionId),
				takeComposerPermissionForSession(sessionId),
			]).toEqual([id, undefined])
		}
	})
})
