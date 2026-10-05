/**
 * Native-item session metrics (rough cut).
 * Cost/token fields stay 0 until Native envelopes carry usage.
 */
import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import { isUserMessageItem, nativeItemType } from "@devo-ai/sdk/v2/client"
import {
	formatCost,
	formatPercentage,
	formatTokens,
	formatWorkDuration,
	type SessionMetricsExtended,
} from "../../lib/session-metrics"
import { itemsFamily } from "../messages"

export interface SessionMetricsValue {
	raw: SessionMetricsExtended
	workTime: string
	cost: string
	tokens: string
	workTimeMs: number
	completedWorkTimeMs: number
	activeStartMs: number | null
	costRaw: number
	tokensRaw: number
	exchangeCount: number
	userMessageCount: number
	assistantMessageCount: number
	modelDistribution: Record<string, number>
	modelDistributionDisplay: Array<{ name: string; count: number }>
	cacheEfficiency: number
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
		cost: 0,
		tokens: { input: 0, output: 0, reasoning: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		exchangeCount: 0,
		userMessageCount: 0,
		assistantMessageCount: 0,
		modelDistribution: {},
		cacheEfficiency: 0,
		errorCount: 0,
		avgExchangeCost: 0,
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
		const raw = emptyRaw({
			userMessageCount,
			assistantMessageCount,
			exchangeCount: userMessageCount,
			toolCallCount,
		})
		return {
			raw,
			workTime: formatWorkDuration(0),
			cost: formatCost(0),
			tokens: formatTokens(0),
			workTimeMs: 0,
			completedWorkTimeMs: 0,
			activeStartMs: null,
			costRaw: 0,
			tokensRaw: 0,
			exchangeCount: userMessageCount,
			userMessageCount,
			assistantMessageCount,
			modelDistribution: {},
			modelDistributionDisplay: [],
			cacheEfficiency: 0,
			cacheEfficiencyFormatted: formatPercentage(0),
			errorCount: 0,
			retryCount: 0,
			toolCallCount,
			toolBreakdown: {},
			avgExchangeCost: formatCost(0),
			avgExchangeTime: formatWorkDuration(0),
		}
	})
})
