import { describe, expect, mock, test } from "bun:test"

// The updater's Electron import must not start a real app in this test.
mock.module("electron", () => ({
	app: { isPackaged: false, getPath: () => "/Applications/Devo.app/Contents/MacOS/Devo" },
	BrowserWindow: { getAllWindows: () => [] },
	shell: { openExternal: async () => {} },
}))

const { detectCanAutoInstall } = await import("./updater")

const executablePath = "/Applications/Devo app/Contents/MacOS/Devo"

describe("macOS updater signing detection", () => {
	test("allows a packaged macOS app when codesign verifies its executable", () => {
		const runCodesign = mock(() => "")
		expect(
			detectCanAutoInstall({
				platform: "darwin",
				packaged: true,
				getExecutablePath: () => executablePath,
				runCodesign,
			}),
		).toBe(true)
		expect(runCodesign).toHaveBeenCalledTimes(1)
		expect(runCodesign).toHaveBeenCalledWith(
			"codesign",
			["--verify", "--deep", "--strict", executablePath],
			{ stdio: "pipe" },
		)
	})

	test("rejects an unsigned packaged macOS app (or unavailable codesign)", () => {
		const runCodesign = mock(() => {
			throw new Error("signature verification failed")
		})
		expect(detectCanAutoInstall({ platform: "darwin", packaged: true, runCodesign })).toBe(
			false,
		)
		expect(runCodesign).toHaveBeenCalledTimes(1)
	})

	test("fails closed if the packaged macOS executable cannot be located", () => {
		const runCodesign = mock(() => "")
		expect(
			detectCanAutoInstall({
				platform: "darwin",
				packaged: true,
				getExecutablePath: () => {
					throw new Error("no executable")
				},
				runCodesign,
			}),
		).toBe(false)
		expect(runCodesign).not.toHaveBeenCalled()
	})

	test("does not invoke codesign outside packaged macOS", () => {
		for (const [platform, packaged] of [
			["linux", true],
			["win32", true],
			["darwin", false],
		] as const) {
			const getExecutablePath = mock(() => executablePath)
			const runCodesign = mock(() => "")
			expect(
				detectCanAutoInstall({ platform, packaged, getExecutablePath, runCodesign }),
			).toBe(true)
			expect(getExecutablePath).not.toHaveBeenCalled()
			expect(runCodesign).not.toHaveBeenCalled()
		}
	})
})
