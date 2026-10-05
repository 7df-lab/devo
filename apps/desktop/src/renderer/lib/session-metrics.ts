/**
 * Session timing / occupancy helpers for the Native transcript path.
 *
 * Cost and token totals stay at zero until Native ItemEnvelope carries usage.
 * No OpenCode Message/Part duals.
 */

import type { NativeItemEnvelope } from "@devo-ai/sdk/v2/client"
import type { ChatTurn } from "../atoms/derived/session-chat"

// ============================================================
// Types
// ============================================================

export interface SessionTokens {
	input: number
	output: number
	reasoning: number
	cacheRead: number
	cacheWrite: number
	total: number
}

/** Distribution of turns across models (modelID -> count) */
export type ModelDistribution = Record<string, number>

/** Distribution of tool calls across categories */
export type ToolBreakdown = Record<string, number>

export interface SessionMetrics {
	/** Total agent work time in milliseconds */
	workTimeMs: number
	/** Work time from completed messages only (excludes in-progress) */
	completedWorkTimeMs: number
	/** Start time (epoch ms) of the in-progress assistant message, or null if idle */
	activeStartMs: number | null
	/** Total cost in USD */
	cost: number
	/** Aggregated token counts */
	tokens: SessionTokens
	/** Number of exchanges (one exchange = one user message + all its assistant responses) */
	exchangeCount: number
	/** Number of user messages in the session */
	userMessageCount: number
	/** Number of assistant messages (LLM invocations) in the session */
	assistantMessageCount: number
	/** Map of modelID -> number of assistant messages using that model */
	modelDistribution: ModelDistribution
	/** Cache hit ratio: cacheRead / (input + cacheRead), as a percentage 0-100 */
	cacheEfficiency: number
	/** Number of assistant messages that had an error */
	errorCount: number
	/** Average cost per exchange (USD) */
	avgExchangeCost: number
	/** Average work time per exchange (ms) */
	avgExchangeTimeMs: number
}

/** Extended metrics including tool occupancy counts */
export interface SessionMetricsExtended extends SessionMetrics {
	/** Tool calls by category (explore, edit, run, delegate, etc.) */
	toolBreakdown: ToolBreakdown
	/** Total number of tool calls */
	toolCallCount: number
	/** Number of retry attempts */
	retryCount: number
}

const MAX_COMPLETED_TURN_WORK_TIME_MS = 24 * 60 * 60 * 1000
const MIN_PLAUSIBLE_EPOCH_MS = Date.UTC(2020, 0, 1)
const MAX_FUTURE_SKEW_MS = 5 * 60 * 1000

function isPlausibleEpochMs(timestamp: number, nowMs: number): boolean {
	return (
		Number.isFinite(timestamp) &&
		timestamp >= MIN_PLAUSIBLE_EPOCH_MS &&
		timestamp <= nowMs + MAX_FUTURE_SKEW_MS
	)
}

function envelopeTimeMs(value: unknown): number | undefined {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value === "string") {
		const ms = Date.parse(value)
		return Number.isFinite(ms) ? ms : undefined
	}
	return undefined
}

function completedTurnEnd(turn: ChatTurn): number | undefined {
	for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
		const completed = envelopeTimeMs(turn.assistantMessages[i].info.updatedAt)
		if (completed !== undefined && turn.assistantMessages[i].info.state === "completed") {
			return completed
		}
	}

	let fallback: number | undefined
	for (const entry of turn.assistantMessages) {
		const created = envelopeTimeMs(entry.info.createdAt)
		const updated = envelopeTimeMs(entry.info.updatedAt)
		for (const timestamp of [updated, created]) {
			if (timestamp !== undefined) {
				fallback = fallback === undefined ? timestamp : Math.max(fallback, timestamp)
			}
		}
	}
	return fallback
}

function completedTurnWorkEnd(turn: ChatTurn): number | undefined {
	let end: number | undefined

	for (const entry of turn.assistantMessages) {
		if (entry.info.state !== "completed" && entry.info.state !== "failed") continue
		const updated = envelopeTimeMs(entry.info.updatedAt)
		if (updated !== undefined) {
			end = end === undefined ? updated : Math.max(end, updated)
		}
	}

	if (end !== undefined) return end
	return completedTurnEnd(turn)
}

function completedTurnStopTime(turn: ChatTurn): number | undefined {
	for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
		const info = turn.assistantMessages[i].info
		if (info.state === "completed" || info.state === "failed" || info.state === "interrupted") {
			const completed = envelopeTimeMs(info.updatedAt)
			if (completed !== undefined) return completed
		}
	}
	return undefined
}

/**
 * Compute end-to-end elapsed time for a single turn.
 * Starts at the user message creation time and ends at the turn completion time.
 * Active turns may use `Date.now()`; completed turns fall back to persisted
 * envelope timestamps so historical durations do not keep growing.
 */
export function computeTurnWorkTime(
	turn: ChatTurn,
	options: { active?: boolean; now?: () => number } = {},
): number {
	const start = envelopeTimeMs(turn.userMessage.info.createdAt)
	const end = options.active ? (options.now?.() ?? Date.now()) : completedTurnWorkEnd(turn)
	if (typeof start !== "number" || typeof end !== "number") return 0
	const elapsed = Math.max(0, end - start)
	if (!options.active && elapsed > MAX_COMPLETED_TURN_WORK_TIME_MS) return 0
	return elapsed
}

/**
 * Compute elapsed time for a single reasoning/thought interval.
 * Active thoughts use `Date.now()`; completed thoughts require both
 * `time.start` and `time.end`.
 */
export function computeThoughtWorkTime(
	part: { time?: { start?: number; end?: number } },
	options: { active?: boolean; now?: () => number } = {},
): number {
	const start = part.time?.start
	if (typeof start !== "number" || !Number.isFinite(start)) return 0
	const end = options.active
		? (options.now?.() ?? Date.now())
		: typeof part.time?.end === "number" && Number.isFinite(part.time.end)
			? part.time.end
			: undefined
	if (typeof end !== "number") return 0
	const elapsed = Math.max(0, end - start)
	if (!options.active && elapsed > MAX_COMPLETED_TURN_WORK_TIME_MS) return 0
	if (!options.active && elapsed <= 0) return 0
	return elapsed
}

/**
 * Compute the live elapsed-time split for a single active turn.
 * The submit button timer uses the same user-message start as completed turn
 * duration, so active and completed displays share one semantic.
 */
export function computeTurnWorkTimeSplit(turn: ChatTurn): {
	completedMs: number
	activeStartMs: number | null
} {
	return { completedMs: 0, activeStartMs: envelopeTimeMs(turn.userMessage.info.createdAt) ?? null }
}

export type LatestTurnTimerMode = "running" | "stopped"

/**
 * Compute the top-bar timer split for the latest turn only.
 * This intentionally ignores earlier turns so each user message resets the
 * app-bar timer, while session-level metrics can still remain cumulative.
 */
export function computeLatestTurnTimerSplit(
	turns: ChatTurn[],
	options: {
		mode: LatestTurnTimerMode
		now?: () => number
		fallbackCompletedMs?: number
	},
): { completedMs: number; activeStartMs: number | null } {
	const turn = turns.at(-1)
	if (!turn) return { completedMs: 0, activeStartMs: null }

	switch (options.mode) {
		case "running": {
			const start = envelopeTimeMs(turn.userMessage.info.createdAt)
			const now = options.now?.() ?? Date.now()
			if (typeof start === "number" && isPlausibleEpochMs(start, now)) {
				return { completedMs: 0, activeStartMs: start }
			}
			return { completedMs: 0, activeStartMs: null }
		}
		case "stopped": {
			const fallback =
				typeof options.fallbackCompletedMs === "number" &&
				Number.isFinite(options.fallbackCompletedMs)
					? Math.max(0, options.fallbackCompletedMs)
					: 0
			const start = envelopeTimeMs(turn.userMessage.info.createdAt)
			const end = completedTurnStopTime(turn)
			if (typeof start === "number" && typeof end === "number") {
				const completedMs = Math.max(0, end - start)
				if (completedMs <= MAX_COMPLETED_TURN_WORK_TIME_MS) {
					return { completedMs, activeStartMs: null }
				}
			}
			return {
				completedMs: fallback,
				activeStartMs: null,
			}
		}
	}
}

/**
 * Compute the cost for a single turn.
 * Native envelopes do not carry cost yet — returns 0 until usage lands on items.
 */
export function computeTurnCost(_turn: ChatTurn): number {
	return 0
}

// ============================================================
// Context window usage (stub until Native usage lands)
// ============================================================

export interface ContextUsage {
	/** Total tokens from the last assistant message */
	lastMessageTokens: number
	/** Model context window limit (from provider data) */
	contextLimit: number
	/** Usage percentage 0-100 */
	percentage: number
	/** Provider ID of the last assistant message */
	providerID: string
	/** Model ID of the last assistant message */
	modelID: string
	/** Token count at which compaction will trigger (null if unknown) */
	compactionThreshold: number | null
	/** Usage percentage toward compaction threshold 0-100 (null if unknown) */
	compactionPercentage: number | null
}

/** Model limit info returned by the lookup callback. */
export interface ModelLimitInfo {
	context: number
	input?: number
	output: number
}

/** Compaction config from the Devo server, used to compute accurate thresholds. */
export interface CompactionOptions {
	/** Whether auto-compaction is enabled (default: true) */
	auto?: boolean
	/** User-configured reserved token buffer (overrides the default 20k) */
	reserved?: number
}

/** Default buffer reserved for output tokens before compaction. */
const COMPACTION_BUFFER = 20_000

/** Maximum output tokens Devo will request (capped at this value). */
const OUTPUT_TOKEN_MAX = 32_000

/**
 * Compute the compaction threshold for a model, mirroring the logic from
 * devo's `SessionCompaction.isOverflow`.
 */
export function computeCompactionThreshold(
	limit: { context: number; input?: number; output: number },
	configReserved?: number,
): number {
	const maxOutput = Math.min(limit.output, OUTPUT_TOKEN_MAX) || OUTPUT_TOKEN_MAX
	const reserved = configReserved ?? Math.min(COMPACTION_BUFFER, maxOutput)
	return limit.input ? limit.input - reserved : limit.context - maxOutput
}

/**
 * Context usage from Native items. Returns null until envelopes carry token
 * usage (live occupancy currently shows counts only).
 */
export function computeContextUsage(
	_items: NativeItemEnvelope[],
	_getModelLimit: (providerID: string, modelID: string) => ModelLimitInfo | undefined,
	_compaction?: CompactionOptions,
): ContextUsage | null {
	return null
}

// ============================================================
// Formatters
// ============================================================

/** Format milliseconds as a compact duration string: "1s", "12s", "1m 34s", "2h 5m". */
export function formatWorkDuration(ms: number): string {
	if (ms <= 0) return "0s"
	const seconds = Math.ceil(ms / 1000)
	if (seconds < 60) return `${seconds}s`
	const minutes = Math.floor(seconds / 60)
	const remainingSeconds = seconds % 60
	if (minutes < 60) {
		return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`
	}
	const hours = Math.floor(minutes / 60)
	const remainingMinutes = minutes % 60
	return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`
}

/** Format a USD cost value: "$0.00", "$0.12", "$1.23". */
export function formatCost(cost: number): string {
	if (cost < 0.005) return "$0.00"
	return `$${cost.toFixed(2)}`
}

/** Format a token count with compact notation: "0", "1.2k", "45.3k", "1.2M". */
export function formatTokens(count: number): string {
	if (count < 1000) return `${Math.round(count)}`
	if (count < 1_000_000) {
		const k = count / 1000
		return k >= 10 ? `${Math.round(k)}k` : `${k.toFixed(1)}k`
	}
	const m = count / 1_000_000
	return m >= 10 ? `${Math.round(m)}M` : `${m.toFixed(1)}M`
}

/** Format a percentage: "0%", "42%", "99.5%". */
export function formatPercentage(pct: number): string {
	if (pct < 0.5) return "0%"
	if (pct >= 99.5) return "100%"
	return pct >= 10 ? `${Math.round(pct)}%` : `${pct.toFixed(1)}%`
}

/**
 * Shorten a model ID for compact display.
 * "claude-sonnet-4-20250514" -> "sonnet-4"
 * "gpt-4o-2024-08-06" -> "gpt-4o"
 * "o3-mini" -> "o3-mini"
 * Falls back to the full ID if no known pattern matches.
 */
export function shortModelName(modelID: string): string {
	if (!modelID) return ""

	// Claude models: claude-{variant}-{version}-{date}
	const claudeMatch = modelID.match(/^claude-(.+?)(-\d{8})?$/)
	if (claudeMatch) return claudeMatch[1]

	// GPT models: gpt-{variant}-{date}
	const gptMatch = modelID.match(/^(gpt-\w+?)(-\d{4}-\d{2}-\d{2})?$/)
	if (gptMatch) return gptMatch[1]

	// Gemini models: gemini-{variant}-{date}
	const geminiMatch = modelID.match(/^(gemini-[\w.-]+?)(-\d{4}-?\d{2})?$/)
	if (geminiMatch) return geminiMatch[1]

	// Generic: strip trailing date patterns (YYYYMMDD or YYYY-MM-DD)
	const genericMatch = modelID.match(/^(.+?)(-\d{8}|-\d{4}-\d{2}-\d{2})$/)
	if (genericMatch) return genericMatch[1]

	return modelID
}
