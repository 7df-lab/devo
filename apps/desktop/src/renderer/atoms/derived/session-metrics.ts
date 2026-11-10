/**
 * Native transcript activity plus cumulative provider metering.
 */
import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import { isUserMessageItem, nativeItemType, type SessionUsageSummary } from "@devo-ai/sdk/v2/client"
import {
	formatPercentage,
	formatTokens,
	formatWorkDuration,
	type SessionMetricsExtended,
} from "../../lib/session-metrics"
import { itemsFamily } from "../messages"
import { sessionNativeFamily } from "../session-native"

export interface SessionMetricsValue {
	raw: SessionMetricsExtended
	usage?: SessionUsageSummary
	workTime: string
	cost: string
	tokens: string
	workTimeMs: number
	completedWorkTimeMs: number
	activeStartMs: number | null
	costRaw: number | null
	tokensRaw: number | null
	exchangeCount: number
	userMessageCount: number
	assistantMessageCount: number
	modelDistribution: Record<string, number>
	modelDistributionDisplay: Array<{ name: string; count: number }>
	cacheEfficiency: number | null
	cacheEfficiencyFormatted: string
	errorCount: number
	retryCount: number
	toolCallCount: number
	toolBreakdown: Record<string, number>
	avgExchangeCost: string
	avgExchangeTime: string
}

function emptyRaw(partial: Partial<SessionMetricsExtended> = {}): SessionMetricsExtended {
	return {
		workTimeMs: 0,
		completedWorkTimeMs: 0,
		activeStartMs: null,
		cost: null,
		tokens: { input: null, output: null, reasoning: null, cacheRead: null, cacheWrite: null, total: null },
		exchangeCount: 0,
		userMessageCount: 0,
		assistantMessageCount: 0,
		modelDistribution: {},
		cacheEfficiency: null,
		errorCount: 0,
		avgExchangeCost: null,
		avgExchangeTimeMs: 0,
		toolBreakdown: {},
		toolCallCount: 0,
		retryCount: 0,
		...partial,
	}
}

export const sessionMetricsFamily = atomFamily((sessionId: string) => {
	return atom((get): SessionMetricsValue => {
		const items = get(itemsFamily(sessionId))
		let userMessageCount = 0
		let assistantMessageCount = 0
		let toolCallCount = 0
		for (const item of items) {
			const type = nativeItemType(item)
			if (isUserMessageItem(item)) userMessageCount += 1
			else if (type === "assistantMessage") assistantMessageCount += 1
			else if (
				type === "toolCall" ||
				type === "commandExecution" ||
				type === "fileChange" ||
				type === "hostedToolCall"
			) {
				toolCallCount += 1
			}
		}
		const usage = get(sessionNativeFamily(sessionId)).cumulativeUsage
		const cost = usage?.estimatedCost ? usage.estimatedCost.micros / 1_000_000 : null
		const formatMoney = (amount: number | null) => amount === null || !usage?.estimatedCost
			? "Unknown"
			: new Intl.NumberFormat("en-US", { style: "currency", currency: usage.estimatedCost.currency }).format(amount)
		const input = usage?.inputTokens ?? null
		const cacheRead = usage?.cacheReadInputTokens ?? null
		const cacheEfficiency = input !== null && input > 0 && cacheRead !== null ? cacheRead / input * 100 : null
		const avgExchangeCost = cost !== null && userMessageCount > 0 ? cost / userMessageCount : null
		const raw = emptyRaw({
			cost,
			tokens: {
				input, output: usage?.outputTokens ?? null, reasoning: usage?.reasoningTokens ?? null,
				cacheRead, cacheWrite: usage?.cacheCreationInputTokens ?? null, total: usage?.totalTokens ?? null,
			},
			cacheEfficiency,
			avgExchangeCost,
			userMessageCount,
			assistantMessageCount,
			exchangeCount: userMessageCount,
			toolCallCount,
		})
		return {
			raw,
			usage,
			workTime: formatWorkDuration(0),
			cost: formatMoney(cost),
			tokens: usage?.totalTokens == null ? "Unknown" : formatTokens(usage.totalTokens),
			workTimeMs: 0,
			completedWorkTimeMs: 0,
			activeStartMs: null,
			costRaw: cost,
			tokensRaw: usage?.totalTokens ?? null,
			exchangeCount: userMessageCount,
			userMessageCount,
			assistantMessageCount,
			modelDistribution: {},
			modelDistributionDisplay: [],
			cacheEfficiency,
			cacheEfficiencyFormatted: cacheEfficiency === null ? "Unknown" : formatPercentage(cacheEfficiency),
			errorCount: 0,
			retryCount: 0,
			toolCallCount,
			toolBreakdown: {},
			avgExchangeCost: formatMoney(avgExchangeCost),
			avgExchangeTime: formatWorkDuration(0),
		}
	})
})
