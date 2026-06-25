import { describe, expect, test } from "bun:test"
import { satisfies } from "semver"
import { DEVO_COMPAT } from "./compatibility"

describe("DEVO_COMPAT", () => {
	test("supports Devo 0.1.21 as the minimum CLI version", () => {
		expect(satisfies("0.1.21", DEVO_COMPAT.supported)).toBe(true)
		expect(satisfies("0.1.21", DEVO_COMPAT.tested)).toBe(true)
		expect(satisfies("0.1.20", DEVO_COMPAT.supported)).toBe(false)
	})
})
