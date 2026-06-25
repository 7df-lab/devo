/**
 * Pure conversion functions between RRULE strings and UI-friendly schedule config.
 *
 * The SchedulePicker component uses ScheduleConfig internally, and these
 * functions convert to/from the RRULE string stored in automation configs.
 *
 * Also provides schedule presets for common patterns and a client-side
 * next-runs preview using the rrule library.
 */

// ============================================================
// Types
// ============================================================

export type ScheduleMode = "daily" | "interval"

/**
 * Unit for interval-mode schedules.
 * "minutes" = FREQ=MINUTELY, "hours" = FREQ=HOURLY.
 */
export type IntervalUnit = "minutes" | "hours"

export type Weekday = "MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU"

export const ALL_WEEKDAYS: Weekday[] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]

export const WEEKDAY_LABELS: Record<Weekday, string> = {
	MO: "Mo",
	TU: "Tu",
	WE: "We",
	TH: "Th",
	FR: "Fr",
	SA: "Sa",
	SU: "Su",
}

export interface ScheduleConfig {
	mode: ScheduleMode
	/** Time in HH:MM format (24h), used for daily mode */
	time: string
	/** Number of interval units between runs, used for interval mode */
	intervalValue: number
	/** Unit for the interval (minutes or hours), used for interval mode */
	intervalUnit: IntervalUnit
	/** Active weekdays */
	weekdays: Weekday[]
}

// ============================================================
// Schedule presets
// ============================================================

export interface SchedulePreset {
	key: string
	label: string
	rrule: string
}

export const SCHEDULE_PRESETS: SchedulePreset[] = [
	{ key: "daily-9am", label: "Every day at 9:00 AM", rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0" },
	{
		key: "weekdays-9am",
		label: "Weekdays at 9:00 AM",
		rrule: "FREQ=DAILY;BYHOUR=9;BYMINUTE=0;BYDAY=MO,TU,WE,TH,FR",
	},
	{ key: "every-30m", label: "Every 30 minutes", rrule: "FREQ=MINUTELY;INTERVAL=30" },
	{ key: "every-1h", label: "Every hour", rrule: "FREQ=HOURLY;INTERVAL=1" },
	{ key: "every-6h", label: "Every 6 hours", rrule: "FREQ=HOURLY;INTERVAL=6" },
	{ key: "every-12h", label: "Every 12 hours", rrule: "FREQ=HOURLY;INTERVAL=12" },
]

/** Special sentinel value for custom schedules that don't match any preset. */
export const CUSTOM_PRESET_KEY = "__custom__"

/**
 * Normalize an RRULE string for comparison: uppercase, sorted parts,
 * strip RRULE: prefix, remove INTERVAL=1 (default).
 */
function normalizeRrule(rrule: string): string {
	const clean = rrule.replace(/^RRULE:/i, "").toUpperCase()
	const parts = clean.split(";").filter(Boolean)

	// Normalize BYDAY: sort the weekdays
	const normalized = parts.map((part) => {
		if (part.startsWith("BYDAY=")) {
			const days = part.slice(6).split(",").sort()
			return `BYDAY=${days.join(",")}`
		}
		return part
	})

	return normalized.sort().join(";")
}

/**
 * Find the preset key that matches the given RRULE string.
 * Returns the preset key or CUSTOM_PRESET_KEY if no match.
 */
export function matchPreset(rrule: string): string {
	const needle = normalizeRrule(rrule)
	for (const preset of SCHEDULE_PRESETS) {
		if (normalizeRrule(preset.rrule) === needle) return preset.key
	}
	return CUSTOM_PRESET_KEY
}

/**
 * Get the label to display for a preset selector given the current RRULE.
 * Returns the preset label or a human-readable summary for custom schedules.
 */
export function getPresetLabel(rrule: string): string {
	const key = matchPreset(rrule)
	if (key !== CUSTOM_PRESET_KEY) {
		const preset = SCHEDULE_PRESETS.find((p) => p.key === key)
		if (preset) return preset.label
	}
	return formatScheduleSummary(rruleToScheduleConfig(rrule))
}

// ============================================================
// Defaults
// ============================================================

export const DEFAULT_SCHEDULE_CONFIG: ScheduleConfig = {
	mode: "daily",
	time: "09:00",
	intervalValue: 24,
	intervalUnit: "hours",
	weekdays: [...ALL_WEEKDAYS],
}

// ============================================================
// Conversion: ScheduleConfig -> RRULE string
// ============================================================

export function scheduleConfigToRrule(config: ScheduleConfig): string {
	const parts: string[] = []

	if (config.mode === "daily") {
		parts.push("FREQ=DAILY")
		const [hours, minutes] = config.time.split(":").map(Number)
		parts.push(`BYHOUR=${hours}`)
		parts.push(`BYMINUTE=${minutes}`)
	} else {
		// Interval mode: FREQ=MINUTELY or FREQ=HOURLY
		if (config.intervalUnit === "minutes") {
			parts.push("FREQ=MINUTELY")
		} else {
			parts.push("FREQ=HOURLY")
		}
		parts.push(`INTERVAL=${config.intervalValue}`)
	}

	// Add weekday filter if not all days are selected
	if (config.weekdays.length > 0 && config.weekdays.length < 7) {
		parts.push(`BYDAY=${config.weekdays.join(",")}`)
	}

	return parts.join(";")
}

// ============================================================
// Conversion: RRULE string -> ScheduleConfig
// ============================================================

export function rruleToScheduleConfig(rrule: string): ScheduleConfig {
	const params = parseRruleParams(rrule)

	const freq = params.FREQ?.toUpperCase() ?? "DAILY"
	const byDay = params.BYDAY
		? (params.BYDAY.split(",").map((d) => d.trim().toUpperCase()) as Weekday[])
		: [...ALL_WEEKDAYS]

	if (freq === "MINUTELY") {
		return {
			mode: "interval",
			time: "09:00",
			intervalValue: params.INTERVAL ? Number.parseInt(params.INTERVAL, 10) : 30,
			intervalUnit: "minutes",
			weekdays: byDay,
		}
	}

	if (freq === "HOURLY") {
		return {
			mode: "interval",
			time: "09:00",
			intervalValue: params.INTERVAL ? Number.parseInt(params.INTERVAL, 10) : 1,
			intervalUnit: "hours",
			weekdays: byDay,
		}
	}

	// Daily mode (or other FREQ treated as daily)
	const hour = params.BYHOUR ? Number.parseInt(params.BYHOUR, 10) : 9
	const minute = params.BYMINUTE ? Number.parseInt(params.BYMINUTE, 10) : 0
	const time = `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`

	return {
		mode: "daily",
		time,
		intervalValue: 24,
		intervalUnit: "hours",
		weekdays: byDay,
	}
}

// ============================================================
// Human-readable schedule summary
// ============================================================

export function formatScheduleSummary(config: ScheduleConfig): string {
	if (config.mode === "interval") {
		const v = config.intervalValue
		const weekdayDesc = formatWeekdayDesc(config.weekdays)

		let base: string
		if (config.intervalUnit === "minutes") {
			base = v === 1 ? "Every minute" : `Every ${v} minutes`
		} else {
			base = v === 1 ? "Every hour" : `Every ${v} hours`
		}

		return weekdayDesc ? `${base}, ${weekdayDesc}` : base
	}

	// Daily mode
	const allDays = config.weekdays.length === 7
	const weekdaysOnly =
		config.weekdays.length === 5 &&
		["MO", "TU", "WE", "TH", "FR"].every((d) => config.weekdays.includes(d as Weekday))
	const weekendsOnly =
		config.weekdays.length === 2 && config.weekdays.includes("SA") && config.weekdays.includes("SU")

	const time12 = formatTime12h(config.time)

	if (allDays) return `Every day at ${time12}`
	if (weekdaysOnly) return `Weekdays at ${time12}`
	if (weekendsOnly) return `Weekends at ${time12}`

	const dayLabels = config.weekdays.map((d) => WEEKDAY_LABELS[d])
	return `${dayLabels.join(", ")} at ${time12}`
}

// ============================================================
// Client-side next runs preview
// ============================================================

/**
 * Compute the next N occurrences for an RRULE string, entirely client-side.
 * Returns ISO date strings. Falls back to empty array on parse errors.
 */
export async function computeNextRuns(rruleStr: string, count = 3): Promise<Date[]> {
	try {
		const rruleModule = await import("rrule")
		// Handle CJS/ESM interop: named export in CJS, nested under default in ESM
		const RRule = rruleModule.RRule ?? rruleModule.default?.RRule
		const rule = RRule.fromString(rruleStr)
		const dates: Date[] = []
		let current = new Date()
		for (let i = 0; i < count; i++) {
			const next = rule.after(current, false)
			if (!next) break
			dates.push(next)
			current = next
		}
		return dates
	} catch {
		return []
	}
}

/**
 * Format a Date for the next-runs preview. Shows day name, date, and time.
 * Example: "Mon, Feb 16 at 9:00 AM"
 */
export function formatNextRun(date: Date): string {
	return date.toLocaleDateString("en-US", {
		weekday: "short",
		month: "short",
		day: "numeric",
		hour: "numeric",
		minute: "2-digit",
		hour12: true,
	})
}

// ============================================================
// Helpers
// ============================================================

function parseRruleParams(rrule: string): Record<string, string> {
	const result: Record<string, string> = {}
	// Strip leading "RRULE:" if present
	const clean = rrule.replace(/^RRULE:/i, "")
	for (const part of clean.split(";")) {
		const eqIdx = part.indexOf("=")
		if (eqIdx > 0) {
			const key = part.slice(0, eqIdx).toUpperCase()
			const value = part.slice(eqIdx + 1)
			result[key] = value
		}
	}
	return result
}

/** Convert 24h time string to 12h format: "09:00" -> "9:00 AM" */
function formatTime12h(time24: string): string {
	const [h, m] = time24.split(":").map(Number)
	const period = h >= 12 ? "PM" : "AM"
	const hour12 = h === 0 ? 12 : h > 12 ? h - 12 : h
	return `${hour12}:${String(m).padStart(2, "0")} ${period}`
}

/** Short weekday description for interval mode summaries */
function formatWeekdayDesc(weekdays: Weekday[]): string | null {
	if (weekdays.length === 7 || weekdays.length === 0) return null
	const weekdaysOnly =
		weekdays.length === 5 &&
		["MO", "TU", "WE", "TH", "FR"].every((d) => weekdays.includes(d as Weekday))
	if (weekdaysOnly) return "weekdays only"
	const weekendsOnly = weekdays.length === 2 && weekdays.includes("SA") && weekdays.includes("SU")
	if (weekendsOnly) return "weekends only"
	return weekdays.map((d) => WEEKDAY_LABELS[d]).join(", ")
}
