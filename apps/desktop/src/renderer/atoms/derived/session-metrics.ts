/**
 * Derived Jotai atom for per-session metrics (work time, cost, tokens,
 * model distribution, cache efficiency, tool breakdown, errors, averages).
 *
 * Uses `atomFamily` keyed by session ID so each session computes its metrics
 * independently. Only re-evaluates when the session's message list or parts change.
 */
import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import { getToolCategory } from "../../components/chat/tool-category"
import {
	computeSessionMetricsExtended,
	formatCost,
	formatPercentage,
	formatTokens,
	formatWorkDuration,
	type ModelDistribution,
	type SessionMetricsExtended,
	shortModelName,
	type ToolBreakdown,
} from "../../lib/session-metrics"
import type { Part } from "../../lib/types"
import { messagesFamily } from "../messages"
import { partsFamily, partStorageKey } from "../parts"
import { appStore } from "../store"
import { viewedSessionIdAtom } from "../ui"

// ============================================================
// Types
// ============================================================

export interface SessionMetricsValue {
	/** Raw metrics (for computation) */
	raw: SessionMetricsExtended
	/** Formatted work duration string, e.g. "1m 34s" */
	workTime: string
	/** Formatted cost string, e.g. "$0.45" */
	cost: string
	/** Formatted total token string, e.g. "12.3k" */
	tokens: string
	/** Raw work time in ms (for live-ticking in the sidebar) */
	workTimeMs: number
	/** Work time from completed messages only (for live-ticking base) */
	completedWorkTimeMs: number
	/** Start time (epoch ms) of in-progress assistant message, or null if idle */
	activeStartMs: number | null
	/** Raw cost number (for comparisons) */
	costRaw: number
	/** Raw total token count (for comparisons) */
	tokensRaw: number
	/** Number of exchanges (user message + all assistant responses) */
	exchangeCount: number
	/** Number of user messages */
	userMessageCount: number
	/** Number of assistant messages (LLM invocations) */
	assistantMessageCount: number
	/** Model distribution: short model name -> count */
	modelDistribution: ModelDistribution
	/** Model distribution with short names for display */
	modelDistributionDisplay: Array<{ name: string; count: number }>
	/** Cache efficiency percentage (0-100) */
	cacheEfficiency: number
	/** Formatted cache efficiency, e.g. "42%" */
	cacheEfficiencyFormatted: string
	/** Number of errors */
	errorCount: number
	/** Number of retries */
	retryCount: number
	/** Total tool calls */
	toolCallCount: number
	/** Tool calls by category */
	toolBreakdown: ToolBreakdown
	/** Average cost per exchange, formatted */
	avgExchangeCost: string
	/** Average work time per exchange, formatted */
	avgExchangeTime: string
}

// ============================================================
// Structural equality
// ============================================================

function metricsEqual(prev: SessionMetricsValue | null, next: SessionMetricsValue): boolean {
	if (!prev) return false
	return (
		prev.workTime === next.workTime &&
		prev.cost === next.cost &&
		prev.tokens === next.tokens &&
		prev.workTimeMs === next.workTimeMs &&
		prev.completedWorkTimeMs === next.completedWorkTimeMs &&
		prev.activeStartMs === next.activeStartMs &&
		prev.costRaw === next.costRaw &&
		prev.tokensRaw === next.tokensRaw &&
		prev.exchangeCount === next.exchangeCount &&
		prev.userMessageCount === next.userMessageCount &&
		prev.assistantMessageCount === next.assistantMessageCount &&
		prev.cacheEfficiency === next.cacheEfficiency &&
		prev.errorCount === next.errorCount &&
		prev.retryCount === next.retryCount &&
		prev.toolCallCount === next.toolCallCount &&
		prev.avgExchangeCost === next.avgExchangeCost &&
		prev.avgExchangeTime === next.avgExchangeTime &&
		modelDistEqual(prev.modelDistribution, next.modelDistribution) &&
		toolBreakdownEqual(prev.toolBreakdown, next.toolBreakdown)
	)
}

function modelDistEqual(a: ModelDistribution, b: ModelDistribution): boolean {
	const aKeys = Object.keys(a)
	const bKeys = Object.keys(b)
	if (aKeys.length !== bKeys.length) return false
	for (const key of aKeys) {
		if (a[key] !== b[key]) return false
	}
	return true
}

function toolBreakdownEqual(a: ToolBreakdown, b: ToolBreakdown): boolean {
	const aKeys = Object.keys(a) as Array<keyof ToolBreakdown>
	const bKeys = Object.keys(b) as Array<keyof ToolBreakdown>
	if (aKeys.length !== bKeys.length) return false
	for (const key of aKeys) {
		if (a[key] !== b[key]) return false
	}
	return true
}

// ============================================================
// Per-session metrics atom family
// ============================================================

/**
 * Derives session metrics (work time, cost, tokens, model distribution,
 * cache efficiency, tool breakdown, errors, averages) for a given session ID.
 *
 * **Optimization**: For the currently viewed session, subscribes reactively to
 * each message's `partsFamily` atom so the metrics panel updates in real time.
 * For background sessions (not currently viewed), reads parts via `appStore.get()`
 * without creating Jotai subscriptions. This means background metrics only
 * recompute when the message list changes (new message added), not on every
 * individual part update. The metrics will catch up when the user navigates
 * to that session (which triggers the reactive path).
 *
 * Uses structural equality to prevent downstream re-renders when the
 * formatted values haven't actually changed (e.g., streaming updates
 * that don't push the duration past the next second boundary).
 */
export const sessionMetricsFamily = atomFamily((sessionId: string) => {
	let prev: SessionMetricsValue | null = null
	return atom((get): SessionMetricsValue => {
		const messages = get(messagesFamily(sessionId))
		const viewedSessionId = get(viewedSessionIdAtom)
		const isViewed = viewedSessionId === sessionId

		// Collect all parts across all messages for tool breakdown / retry counting.
		// For the viewed session, use reactive `get()` to subscribe to part changes.
		// For background sessions, use non-reactive `appStore.get()` to avoid
		// creating subscriptions that would trigger recomputation on every part update.
		const allParts: Part[] = []
		for (const msg of messages) {
			const key = partStorageKey(sessionId, msg.id)
			const parts = isViewed ? get(partsFamily(key)) : appStore.get(partsFamily(key))
			if (parts.length > 0) {
				for (const p of parts) {
					allParts.push(p)
				}
			}
		}

		const raw = computeSessionMetricsExtended(messages, allParts, getToolCategory)

		// Build display-friendly model distribution with short names
		const modelDistributionDisplay = Object.entries(raw.modelDistribution)
			.map(([modelID, count]) => ({ name: shortModelName(modelID), count }))
			.sort((a, b) => b.count - a.count)

		const next: SessionMetricsValue = {
			raw,
			workTime: formatWorkDuration(raw.workTimeMs),
			cost: formatCost(raw.cost),
			tokens: formatTokens(raw.tokens.total),
			workTimeMs: raw.workTimeMs,
			completedWorkTimeMs: raw.completedWorkTimeMs,
			activeStartMs: raw.activeStartMs,
			costRaw: raw.cost,
			tokensRaw: raw.tokens.total,
			exchangeCount: raw.exchangeCount,
			userMessageCount: raw.userMessageCount,
			assistantMessageCount: raw.assistantMessageCount,
			modelDistribution: raw.modelDistribution,
			modelDistributionDisplay,
			cacheEfficiency: raw.cacheEfficiency,
			cacheEfficiencyFormatted: formatPercentage(raw.cacheEfficiency),
			errorCount: raw.errorCount,
			retryCount: raw.retryCount,
			toolCallCount: raw.toolCallCount,
			toolBreakdown: raw.toolBreakdown,
			avgExchangeCost: formatCost(raw.avgExchangeCost),
			avgExchangeTime: formatWorkDuration(raw.avgExchangeTimeMs),
		}

		if (metricsEqual(prev, next)) return prev!
		prev = next
		return next
	})
})
