import { describe, expect, test } from "bun:test"
import path from "node:path"
import { resolveDevoProgram } from "./devo-program"

describe("resolveDevoProgram", () => {
	test("prefers the checkout debug CLI in desktop dev mode", () => {
		const appPath = path.join("repo", "apps", "desktop")
		const checkoutDebug = path.resolve(appPath, "..", "..", "target", "debug", "devo")
		const program = resolveDevoProgram({
			appPath,
			env: {},
			existsSync: (candidate) => candidate === checkoutDebug,
			isPackaged: false,
		})

		expect(program).toBe(checkoutDebug)
	})

	if (process.platform === "win32") {
		test("prefers the checkout debug CLI executable in Windows desktop dev mode", () => {
			const program = resolveDevoProgram({
				appPath: "C:\\repo\\apps\\desktop",
				env: {},
				existsSync: (candidate) => candidate === "C:\\repo\\target\\debug\\devo.exe",
				isPackaged: false,
			})

			expect(program).toBe("C:\\repo\\target\\debug\\devo.exe")
		})
	}

	test("uses explicit override before dev checkout candidates", () => {
		const program = resolveDevoProgram({
			appPath: "/repo/apps/desktop",
			env: { DEVO_DESKTOP_DEVO_BIN: "/custom/devo" },
			existsSync: () => true,
			isPackaged: false,
		})

		expect(program).toBe("/custom/devo")
	})

	test("uses bundled runtime in packaged apps", () => {
		const program = resolveDevoProgram({
			appPath: "/repo/apps/desktop",
			env: {},
			existsSync: (candidate) => candidate === "/Applications/Devo.app/Contents/Resources/runtime/bin/devo",
			isPackaged: true,
			resourcesPath: "/Applications/Devo.app/Contents/Resources",
		})

		expect(program).toBe("/Applications/Devo.app/Contents/Resources/runtime/bin/devo")
	})

	test("uses bundled Windows runtime executable in packaged apps", () => {
		const program = resolveDevoProgram({
			appPath: "/app/resources/app.asar",
			env: {},
			existsSync: (candidate) => candidate === "/app/resources/runtime/bin/devo.exe",
			isPackaged: true,
			platform: "win32",
			resourcesPath: "/app/resources",
		})

		expect(program).toBe("/app/resources/runtime/bin/devo.exe")
	})

	test("fails clearly when packaged runtime is missing", () => {
		expect(() =>
			resolveDevoProgram({
				appPath: "/repo/apps/desktop",
				env: {},
				existsSync: () => false,
				isPackaged: true,
				resourcesPath: "/Applications/Devo.app/Contents/Resources",
			}),
		).toThrow("Bundled Devo runtime not found")
	})
})
