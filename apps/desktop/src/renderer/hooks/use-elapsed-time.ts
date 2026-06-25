/**
 * Hook that returns a live-updating elapsed time string for running tool calls.
 *
 * Ticks every second while the tool is running/pending, returning a formatted
 * duration like "3s", "1m 23s". Returns `undefined` when the tool is not active.
 *
 * Uses the SDK tool state's `time.start`, matching the completed tool duration
 * calculation and avoiding mixed client/server timestamp sources.
 */

import { useEffect, useState } from "react"
import type { ToolPart } from "../lib/types"

function formatElapsed(ms: number): string {
	const safeMs = Math.max(0, ms)
	if (safeMs < 1000) return "0s"
	const seconds = Math.floor(safeMs / 1000)
	if (seconds < 60) return `${seconds}s`
	const minutes = Math.floor(seconds / 60)
	const remainingSeconds = seconds % 60
	return `${minutes}m ${remainingSeconds}s`
}

export function useToolElapsedTime(part: ToolPart): string | undefined {
	const status = part.state.status
	const isActive = status === "running" || status === "pending"

	const startTime =
		"time" in part.state ? (part.state.time as { start: number }).start : undefined

	const [elapsed, setElapsed] = useState<string | undefined>(() => {
		if (!isActive || !startTime) return undefined
		return formatElapsed(Date.now() - startTime)
	})

	useEffect(() => {
		if (!isActive || !startTime) {
			setElapsed(undefined)
			return
		}

		// Compute immediately
		setElapsed(formatElapsed(Date.now() - startTime))

		const intervalId = setInterval(() => {
			setElapsed(formatElapsed(Date.now() - startTime))
		}, 1_000)

		return () => clearInterval(intervalId)
	}, [isActive, startTime])

	return elapsed
}
