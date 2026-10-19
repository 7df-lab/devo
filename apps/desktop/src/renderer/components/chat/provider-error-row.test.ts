import { describe, expect, test } from "bun:test"
import { providerErrorSummary, type ProviderErrorEntry } from "./provider-error-row"

const failure: ProviderErrorEntry = {
	id: "error-1",
	turnId: "turn-1",
	phase: "failed",
	code: "INVALID_REQUEST_ERROR",
	message: "HTTP 402: Insufficient Balance",
}

describe("provider error summary", () => {
	test("shows an actionable balance failure instead of the wire error code", () => {
		expect(providerErrorSummary(failure, 0, false)).toBe("Insufficient provider balance")
	})

	test("keeps other failures readable", () => {
		expect(providerErrorSummary({ ...failure, message: "Invalid model" }, 0, false)).toBe("Request failed")
	})

	test("preserves scheduled retries and countdowns", () => {
		const scheduled = { ...failure, phase: "scheduled", attempt: 2 }
		expect([
			providerErrorSummary(scheduled, 12000, true),
			providerErrorSummary(scheduled, 500, true),
			providerErrorSummary(scheduled, 0, false),
		]).toEqual([
			"Provider retry (attempt 2) · 12s",
			"Provider retry (attempt 2) · 0.5s",
			"Provider retry (attempt 2)",
		])
	})
})
