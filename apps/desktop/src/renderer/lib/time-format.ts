/**
 * Pure time formatting utilities for compact relative timestamps.
 *
 * Used by the automations inbox for "1h", "2h", "3d" style timestamps
 * and "Starts in 32m" style countdowns.
 */

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
const WEEK = 7 * DAY
const MONTH = 30 * DAY
const YEAR = 365 * DAY

/**
 * Format a past timestamp as a compact relative string.
 * Examples: "1m", "5m", "1h", "3h", "2d", "1w", "3mo", "1y"
 */
export function formatTimeAgo(timestamp: number): string {
	const now = Date.now()
	const diff = now - timestamp
	if (diff < 0) return "now"

	if (diff < MINUTE) return "now"
	if (diff < HOUR) return `${Math.floor(diff / MINUTE)}m`
	if (diff < DAY) return `${Math.floor(diff / HOUR)}h`
	if (diff < WEEK) return `${Math.floor(diff / DAY)}d`
	if (diff < MONTH) return `${Math.floor(diff / WEEK)}w`
	if (diff < YEAR) return `${Math.floor(diff / MONTH)}mo`
	return `${Math.floor(diff / YEAR)}y`
}

/**
 * Format a future timestamp as a compact countdown string.
 * Examples: "now", "32m", "1h", "2d", "1w"
 */
export function formatCountdown(futureTimestamp: number): string {
	const now = Date.now()
	const diff = futureTimestamp - now
	if (diff <= 0) return "now"

	if (diff < HOUR) return `${Math.max(1, Math.ceil(diff / MINUTE))}m`
	if (diff < DAY) return `${Math.floor(diff / HOUR)}h`
	if (diff < WEEK) return `${Math.floor(diff / DAY)}d`
	if (diff < MONTH) return `${Math.floor(diff / WEEK)}w`
	if (diff < YEAR) return `${Math.floor(diff / MONTH)}mo`
	return `${Math.floor(diff / YEAR)}y`
}

/**
 * Format a duration in milliseconds as a human-readable string.
 * Examples: "5s", "1m 23s", "1h 5m"
 */
export function formatDuration(ms: number): string {
	if (ms < 1000) return "<1s"
	const totalSeconds = Math.floor(ms / 1000)
	const hours = Math.floor(totalSeconds / 3600)
	const minutes = Math.floor((totalSeconds % 3600) / 60)
	const seconds = totalSeconds % 60

	if (hours > 0) return `${hours}h ${minutes}m`
	if (minutes > 0) return `${minutes}m ${seconds}s`
	return `${seconds}s`
}
