/**
 * Hook that returns a live-updating countdown string for a future timestamp.
 *
 * Re-renders every 60 seconds (or every second when under a minute)
 * so the "Next in 32m" text stays fresh without relying on parent polls.
 */

import { useEffect, useState } from "react"
import { formatCountdown } from "../lib/time-format"

export function useCountdown(futureTimestamp: number | null): string | null {
	const [label, setLabel] = useState(() =>
		futureTimestamp ? formatCountdown(futureTimestamp) : null,
	)

	useEffect(() => {
		if (!futureTimestamp) {
			setLabel(null)
			return
		}

		// Compute immediately
		setLabel(formatCountdown(futureTimestamp))

		function tick() {
			setLabel(formatCountdown(futureTimestamp!))
		}

		// Tick every 30s for general countdowns, every 5s when under 2 minutes
		function getInterval(): number {
			const diff = futureTimestamp! - Date.now()
			if (diff <= 0) return 5_000
			if (diff < 120_000) return 5_000
			return 30_000
		}

		let timerId: ReturnType<typeof setTimeout>

		function schedule() {
			timerId = setTimeout(() => {
				tick()
				schedule()
			}, getInterval())
		}

		schedule()

		return () => clearTimeout(timerId)
	}, [futureTimestamp])

	return label
}
