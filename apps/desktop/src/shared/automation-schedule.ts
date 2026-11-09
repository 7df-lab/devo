/** Calendar recurrence calculation shared by the scheduler and its preview. */

const HOUR_MS = 60 * 60 * 1000

function wallTime(date: Date, formatter: Intl.DateTimeFormat): Date {
	const parts = Object.fromEntries(
		formatter.formatToParts(date).map(({ type, value }) => [type, value]),
	)
	return new Date(
		Date.UTC(
			Number(parts.year),
			Number(parts.month) - 1,
			Number(parts.day),
			Number(parts.hour),
			Number(parts.minute),
			Number(parts.second),
		),
	)
}

/**
 * Resolve a calendar time without depending on the host process's timezone.
 * Missing spring-forward times are skipped; repeated fall-back times run once,
 * at the earlier instant. Check both sides of transitions, including half-hour DST.
 */
function resolveWallTime(wall: Date, formatter: Intl.DateTimeFormat): Date | null {
	const timestamp = wall.getTime()
	const offsets = new Set<number>()
	for (const hours of [-36, 0, 36]) {
		const sample = new Date(timestamp + hours * HOUR_MS)
		offsets.add(wallTime(sample, formatter).getTime() - sample.getTime())
	}
	const matches: number[] = []
	for (const offset of offsets) {
		const candidate = timestamp - offset
		if (wallTime(new Date(candidate), formatter).getTime() === timestamp) {
			matches.push(candidate)
		}
	}
	return matches.length > 0 ? new Date(Math.min(...matches)) : null
}

export async function scheduleOccurrences(
	rruleStr: string,
	timezone: string,
	{
		after = new Date(),
		count = 5,
		startAt = after,
	}: {
		after?: Date
		count?: number
		startAt?: Date
	} = {},
): Promise<Date[]> {
	if (!Number.isInteger(count) || count < 0 || count > 100) {
		throw new RangeError("Schedule preview count must be an integer between 0 and 100")
	}
	// Validate even when count is zero. Invalid zones must never fall back to UTC.
	const formatter = new Intl.DateTimeFormat("en-GB", {
		timeZone: timezone,
		calendar: "gregory",
		numberingSystem: "latn",
		hourCycle: "h23",
		year: "numeric",
		month: "2-digit",
		day: "2-digit",
		hour: "2-digit",
		minute: "2-digit",
		second: "2-digit",
	})
	const rruleModule = await import("rrule")
	const RRule = rruleModule.RRule ?? rruleModule.default?.RRule
	const options = RRule.parseString(rruleStr)
	if (options.tzid && options.tzid !== timezone) {
		throw new Error("RRULE TZID must match the automation timezone")
	}
	const until = options.until
	// rrule's TZID conversion depends on the host timezone. Generate floating
	// wall-clock fields instead and convert them to real instants ourselves.
	const rule = new RRule({
		...options,
		tzid: null,
		until: null,
		dtstart: options.dtstart
			? /DTSTART:\d{8}T\d{6}Z/i.test(rruleStr)
				? wallTime(options.dtstart, formatter)
				: options.dtstart
			: wallTime(startAt, formatter),
	})
	const dates: Date[] = []
	let cursor = wallTime(after, formatter)
	// Include the current wall second, then filter by the actual instant below.
	let inclusive = true
	while (dates.length < count) {
		const next = rule.after(cursor, inclusive)
		if (!next) break
		inclusive = false
		cursor = next
		const instant = resolveWallTime(next, formatter)
		if (instant && until && instant > until) break
		if (instant && instant > after) dates.push(instant)
	}
	return dates
}
