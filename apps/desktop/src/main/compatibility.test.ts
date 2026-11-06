import { describe, expect, test } from "bun:test"
import { satisfies } from "semver"
import { checkDevoProgram, DEVO_COMPAT } from "./compatibility"

describe("DEVO_COMPAT", () => {
	test("supports Devo 0.1.21 as the minimum CLI version", () => {
		expect(satisfies("0.1.21", DEVO_COMPAT.supported)).toBe(true)
		expect(satisfies("0.1.21", DEVO_COMPAT.tested)).toBe(true)
		expect(satisfies("0.1.20", DEVO_COMPAT.supported)).toBe(false)
	})
})

describe("checkDevoProgram", () => {
	test("checks an explicit bundled runtime path without consulting PATH", async () => {
		const result = await checkDevoProgram({
			program: "/Applications/Devo.app/Contents/Resources/runtime/bin/devo",
			env: { PATH: "/usr/bin" },
			execFile: (_cmd, _args, _options, callback) => {
				callback(null, "devo v0.1.22\n")
			},
		})

		expect(result).toEqual({
			installed: true,
			version: "0.1.22",
			path: "/Applications/Devo.app/Contents/Resources/runtime/bin/devo",
			compatible: true,
			compatibility: "ok",
			message: null,
		})
	})
})

test("a failed selected runtime check does not fall back to PATH discovery", async () => {
	const calls: Array<{ cmd: string; args: string[] }> = []
	const result = await checkDevoProgram({
		program: "selected-runtime",
		env: { PATH: "other-runtime-directory" },
		execFile: (cmd, args, _options, callback) => {
			calls.push({ cmd, args })
			callback(new Error("Unsupported --version"), "")
		},
	})
	expect(calls).toEqual([{ cmd: "selected-runtime", args: ["--version"] }])
	expect(result).toEqual({
		installed: false,
		version: null,
		path: "selected-runtime",
		compatible: false,
		compatibility: "unknown",
		message: "Devo runtime not found at selected-runtime",
	})
})
