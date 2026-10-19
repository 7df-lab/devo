import { describe, expect, test } from "bun:test"
import path from "node:path"
import { resolveDevoProgram } from "./devo-program"

describe("resolveDevoProgram", () => {
	test("prefers the complete shared runtime bundle", () => {
		const resourcesPath = path.join(process.cwd(), "complete-resources")
		const bundled = path.join(resourcesPath, "runtime", process.platform === "win32" ? "devo.exe" : "devo")
		expect(resolveDevoProgram({ appPath: process.cwd(), env: {}, isPackaged: true, resourcesPath, existsSync: () => true })).toBe(bundled)
	})
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
		const resourcesPath = path.join(process.cwd(), "packaged-resources")
		const bundled = path.join(resourcesPath, "runtime", "bin", process.platform === "win32" ? "devo.exe" : "devo")
		const program = resolveDevoProgram({
			appPath: "/repo/apps/desktop",
			env: {},
			existsSync: (candidate) => candidate === bundled,
			isPackaged: true,
			resourcesPath,
		})

		expect(program).toBe(bundled)
	})

	test("uses bundled Windows runtime executable in packaged apps", () => {
		const resourcesPath = path.join(process.cwd(), "windows-resources")
		const bundled = path.join(resourcesPath, "runtime", "bin", "devo.exe")
		const program = resolveDevoProgram({
			appPath: "/app/resources/app.asar",
			env: {},
			existsSync: (candidate) => candidate === bundled,
			isPackaged: true,
			platform: "win32",
			resourcesPath,
		})

		expect(program).toBe(bundled)
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
