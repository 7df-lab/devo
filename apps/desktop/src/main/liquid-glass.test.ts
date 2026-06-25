import { describe, expect, test } from "bun:test"
import {
	getResolvedChromeTier,
	resolveWindowChrome,
	resolveWindowsTitleBarOverlay,
} from "./liquid-glass"

describe("resolveWindowChrome", () => {
	test("uses transparent acrylic chrome on Windows when opaque windows are disabled", async () => {
		const chrome = await resolveWindowChrome({
			isOpaque: false,
			isDarkMode: true,
			platform: "win32",
		})

		expect(chrome).toEqual({
			tier: "transparent",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				backgroundMaterial: "acrylic",
				resizable: true,
				maximizable: true,
				minimizable: true,
				fullscreenable: true,
				thickFrame: true,
				roundedCorners: true,
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#00000000",
					symbolColor: "#f4f4f5",
					height: 40,
				},
			},
		})
		expect(getResolvedChromeTier()).toBe("transparent")
	})

	test("honors opaque windows on Windows", async () => {
		const chrome = await resolveWindowChrome({
			isOpaque: true,
			isDarkMode: true,
			platform: "win32",
		})

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#00000000",
					symbolColor: "#f4f4f5",
					height: 40,
				},
			},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("keeps Linux opaque even when opaque windows are disabled", async () => {
		const chrome = await resolveWindowChrome({ isOpaque: false, platform: "linux" })

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("keeps macOS titlebar settings in opaque mode", async () => {
		const chrome = await resolveWindowChrome({ isOpaque: true, platform: "darwin" })

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				titleBarStyle: "hiddenInset",
				trafficLightPosition: { x: 15, y: 15 },
			},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("uses dark titlebar overlay symbols in light mode on Windows", () => {
		expect(resolveWindowsTitleBarOverlay(false)).toEqual({
			color: "#00000000",
			symbolColor: "#111111",
			height: 40,
		})
	})
})
