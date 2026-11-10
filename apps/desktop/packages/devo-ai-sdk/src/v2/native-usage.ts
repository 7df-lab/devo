/** Cumulative provider metering, separate from the latest request's context occupancy. */
export interface SessionUsageSummary {
	totalTokens: number | null
	inputTokens: number | null
	outputTokens: number | null
	reasoningTokens: number | null
	cacheReadInputTokens: number | null
	cacheCreationInputTokens: number | null
	callCount: number | null
	meteredCallCount: number | null
	coverage: "complete" | "partial" | "unreported" | "unknown" | "legacy"
	legacyTokens?: number
	unreportedFields?: string[]
	estimatedCost: { currency: string; micros: number } | null
}

function record(value: unknown): Record<string, unknown> | undefined {
	return value !== null && typeof value === "object" && !Array.isArray(value)
		? value as Record<string, unknown>
		: undefined
}

function count(value: unknown): number | null {
	return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null
}

/** Accept either SessionUsage or its UsageTotals; never replace absent fields with zero. */
export function sessionUsageFromProtocol(value: unknown): SessionUsageSummary | undefined {
	const usage = record(value)
	const total = record(usage?.total) ?? usage
	if (!total || !["totalTokens", "inputTokens", "callCount"].some((key) => key in total)) return undefined
	const callCount = count(total.callCount)
	const meteredCallCount = count(total.meteredCallCount)
	const totalTokens = count(total.totalTokens)
	const hasReportedTokens = (totalTokens ?? 0) > 0 || (meteredCallCount ?? 0) > 0
	const unreported = (callCount ?? 0) > 0 && meteredCallCount === 0 && !hasReportedTokens
	const legacyOnly = (totalTokens ?? 0) > 0 && meteredCallCount === 0
	const legacyTokens = count(record(usage?.legacy)?.totalTokens) ?? (legacyOnly ? totalTokens : 0) ?? 0
	const unreportedFields = Array.isArray(total.unreportedFields) ? total.unreportedFields.filter((field): field is string => typeof field === "string") : undefined
	const field = (key: string) => unreportedFields?.includes(key) ? null : count(total[key])
	const coverage = legacyTokens > 0 ? "legacy" : unreported
		? "unreported"
		: callCount === null || meteredCallCount === null || (callCount === 0 && hasReportedTokens)
			? "unknown"
			: meteredCallCount < callCount || (unreportedFields?.length ?? 0) > 0 ? "partial"
				: callCount > 0 && !unreportedFields ? "unknown" : "complete"
	const cost = record(total.estimatedCost)
	return {
		totalTokens: unreported ? null : field("totalTokens"),
		inputTokens: unreported || legacyOnly ? null : field("inputTokens"),
		outputTokens: unreported || legacyOnly ? null : field("outputTokens"),
		reasoningTokens: unreported || legacyOnly ? null : field("reasoningTokens"),
		cacheReadInputTokens: unreported || legacyOnly ? null : field("cacheReadInputTokens"),
		cacheCreationInputTokens: unreported || legacyOnly ? null : field("cacheCreationInputTokens"),
		callCount,
		meteredCallCount,
		coverage,
		...(legacyTokens > 0 ? { legacyTokens } : {}),
		...(unreportedFields?.length ? { unreportedFields } : {}),
		estimatedCost: cost && typeof cost.currency === "string" && /^[A-Z]{3}$/.test(cost.currency) && count(cost.micros) !== null
			? { currency: cost.currency, micros: cost.micros as number }
			: null,
	}
}

/** Spend cannot decrease when a delayed list/resume snapshot follows a live update. */
export function newerSessionUsage(previous: SessionUsageSummary | undefined, next: SessionUsageSummary): SessionUsageSummary {
	if (!previous) return next
	if (previous.callCount !== null && next.callCount !== null && next.callCount < previous.callCount) return previous
	if (previous.totalTokens !== null && next.totalTokens !== null && next.totalTokens < previous.totalTokens) return previous
	return previous.legacyTokens && !next.legacyTokens
		? { ...next, legacyTokens: previous.legacyTokens, coverage: "legacy" }
		: next
}
