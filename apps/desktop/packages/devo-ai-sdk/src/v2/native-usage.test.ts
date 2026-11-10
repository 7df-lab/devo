import { expect, test } from "bun:test"
import { newerSessionUsage, sessionUsageFromProtocol } from "./native-usage"

const totals = {
	unreportedFields: [],
	totalTokens: 350, inputTokens: 300, outputTokens: 50, reasoningTokens: 10,
	cacheReadInputTokens: 100, cacheCreationInputTokens: 40, callCount: 2, meteredCallCount: 2,
}

test("preserves provider total and treats cache as input subsets", () => {
	expect(sessionUsageFromProtocol({ total: { ...totals, totalTokens: 370 } })).toEqual({
		...Object.fromEntries(Object.entries(totals).filter(([key]) => key !== "unreportedFields")), totalTokens: 370, coverage: "complete", estimatedCost: null,
	})
})

test("missing usage is unknown while a new session has no spend", () => {
	const empty = Object.fromEntries(Object.keys(totals).filter((key) => key !== "unreportedFields").map((key) => [key, 0]))
	expect(sessionUsageFromProtocol(undefined)).toBeUndefined()
	expect(sessionUsageFromProtocol({ ...empty, callCount: 1 })).toEqual({
		totalTokens: null, inputTokens: null, outputTokens: null, reasoningTokens: null,
		cacheReadInputTokens: null, cacheCreationInputTokens: null, callCount: 1, meteredCallCount: 0,
		coverage: "unreported", estimatedCost: null,
	})
	expect(sessionUsageFromProtocol(empty)).toEqual({ ...empty, coverage: "complete", estimatedCost: null })
})

test("partial coverage and absent fields are retained instead of fabricated", () => {
	expect(sessionUsageFromProtocol({ totalTokens: 120, inputTokens: 100, outputTokens: 20, callCount: 2, meteredCallCount: 1 })).toEqual({
		totalTokens: 120, inputTokens: 100, outputTokens: 20, reasoningTokens: null,
		cacheReadInputTokens: null, cacheCreationInputTokens: null, callCount: 2, meteredCallCount: 1,
		coverage: "partial", estimatedCost: null,
	})
})

test("stale snapshots cannot erase live spend or known coverage", () => {
	const current = sessionUsageFromProtocol(totals)!
	const stale = sessionUsageFromProtocol({ ...totals, callCount: 1, meteredCallCount: 1, totalTokens: 120 })!
	expect(newerSessionUsage(current, stale)).toEqual(current)
	expect(newerSessionUsage(current, sessionUsageFromProtocol({ ...totals, totalTokens: 120 })!)).toEqual(current)
	const missingNext = sessionUsageFromProtocol({ ...totals, callCount: 3 })!
	expect(newerSessionUsage(current, missingNext)).toEqual({ ...current, callCount: 3, coverage: "partial" })
})

test("only valid reported cost is exposed", () => {
	expect(sessionUsageFromProtocol({ ...totals, estimatedCost: { currency: "USD", micros: 125000 } })?.estimatedCost).toEqual({ currency: "USD", micros: 125000 })
	expect(sessionUsageFromProtocol({ ...totals, estimatedCost: { currency: "USD", micros: -1 } })?.estimatedCost).toBeNull()
})


test("historical totals do not invent a breakdown, and retain their caveat on live updates", () => {
	const historical = sessionUsageFromProtocol({ ...totals, callCount: 0, meteredCallCount: 0 })!
	expect(historical).toMatchObject({ totalTokens: 350, inputTokens: null, outputTokens: null, cacheReadInputTokens: null, legacyTokens: 350, coverage: "legacy" })
	const mixed = sessionUsageFromProtocol({ total: { ...totals, totalTokens: 700 }, legacy: { totalTokens: 350 } })!
	expect(mixed).toMatchObject({ totalTokens: 700, inputTokens: 300, legacyTokens: 350, coverage: "legacy" })
	expect(newerSessionUsage(mixed, sessionUsageFromProtocol({ ...totals, callCount: 3, totalTokens: 800 })!)).toMatchObject({ totalTokens: 800, legacyTokens: 350, coverage: "legacy" })
})

test("field coverage hides unknown splits while preserving a separately reported total", () => {
	const partial = sessionUsageFromProtocol({ ...totals, inputTokens: 0, outputTokens: 0, unreportedFields: ["inputTokens", "outputTokens", "reasoningTokens", "cacheReadInputTokens", "cacheCreationInputTokens"] })!
	expect(partial).toMatchObject({ totalTokens: 350, inputTokens: null, outputTokens: null, reasoningTokens: null, cacheReadInputTokens: null, cacheCreationInputTokens: null, coverage: "partial" })
	const zero = sessionUsageFromProtocol({ ...totals, inputTokens: 0, outputTokens: 0, totalTokens: 0, unreportedFields: [] })!
	expect(zero).toMatchObject({ totalTokens: 0, inputTokens: 0, outputTokens: 0, coverage: "complete" })
	expect(sessionUsageFromProtocol({ ...totals, unreportedFields: ["totalTokens"] })?.totalTokens).toBeNull()
	expect(sessionUsageFromProtocol({ ...totals, unreportedFields: undefined })?.coverage).toBe("unknown")
})
