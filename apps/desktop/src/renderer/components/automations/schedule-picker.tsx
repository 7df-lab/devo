/**
 * Schedule picker with presets, natural language sentence builder, and
 * live next-runs preview.
 *
 * Externally controlled via RRULE string: value + onChange.
 *
 * Layout:
 * 1. Preset selector (dropdown) -- covers 80% of use cases in one click
 * 2. Custom configuration (sentence builder) -- shown when "Custom" is
 *    selected, or when the current RRULE doesn't match any preset
 * 3. Live summary + next 3 runs -- always shown at the bottom
 */

import { Button } from "@devo/ui/components/button"
import { Input } from "@devo/ui/components/input"
import { Label } from "@devo/ui/components/label"
import {
	Select,
	SelectContent,
	SelectItem,
	SelectSeparator,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { CalendarClockIcon } from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import {
	ALL_WEEKDAYS,
	CUSTOM_PRESET_KEY,
	computeNextRuns,
	formatNextRun,
	formatScheduleSummary,
	type IntervalUnit,
	matchPreset,
	rruleToScheduleConfig,
	SCHEDULE_PRESETS,
	type ScheduleConfig,
	scheduleConfigToRrule,
	WEEKDAY_LABELS,
	type Weekday,
} from "../../lib/rrule-ui"

// ============================================================
// Props
// ============================================================

interface SchedulePickerProps {
	value: string
	onChange: (rrule: string) => void
}

// ============================================================
// Main component
// ============================================================

export function SchedulePicker({ value, onChange }: SchedulePickerProps) {
	const config = useMemo(() => rruleToScheduleConfig(value), [value])
	const presetKey = useMemo(() => matchPreset(value), [value])
	const isCustom = presetKey === CUSTOM_PRESET_KEY

	// Track whether the user explicitly chose "Custom" (vs. auto-detected)
	const [showCustomBuilder, setShowCustomBuilder] = useState(isCustom)

	// Sync: if the rrule matches a preset, hide the custom builder
	useEffect(() => {
		if (!isCustom) setShowCustomBuilder(false)
	}, [isCustom])

	// --- Preset items map for Base UI Select (resolves labels before popup opens)
	const presetItems = useMemo(() => {
		const map: Record<string, string> = {}
		for (const p of SCHEDULE_PRESETS) {
			map[p.key] = p.label
		}
		map[CUSTOM_PRESET_KEY] = "Custom"
		return map
	}, [])

	// --- Preset selection
	const handlePresetChange = useCallback(
		(key: string | null) => {
			if (!key) return
			if (key === CUSTOM_PRESET_KEY) {
				setShowCustomBuilder(true)
				return
			}
			const preset = SCHEDULE_PRESETS.find((p) => p.key === key)
			if (preset) {
				setShowCustomBuilder(false)
				onChange(preset.rrule)
			}
		},
		[onChange],
	)

	// --- Custom config updates
	const updateConfig = useCallback(
		(patch: Partial<ScheduleConfig>) => {
			const updated = { ...config, ...patch }
			onChange(scheduleConfigToRrule(updated))
		},
		[config, onChange],
	)

	const setMode = useCallback(
		(mode: ScheduleConfig["mode"]) => updateConfig({ mode }),
		[updateConfig],
	)
	const setTime = useCallback((time: string) => updateConfig({ time }), [updateConfig])
	const setIntervalValue = useCallback(
		(value: number) => updateConfig({ intervalValue: Math.max(1, value) }),
		[updateConfig],
	)
	const setIntervalUnit = useCallback(
		(unit: IntervalUnit) => updateConfig({ intervalUnit: unit }),
		[updateConfig],
	)
	const toggleWeekday = useCallback(
		(day: Weekday) => {
			const current = new Set(config.weekdays)
			if (current.has(day)) {
				if (current.size <= 1) return
				current.delete(day)
			} else {
				current.add(day)
			}
			updateConfig({ weekdays: ALL_WEEKDAYS.filter((d) => current.has(d)) })
		},
		[config.weekdays, updateConfig],
	)

	// --- Summary
	const summary = useMemo(() => formatScheduleSummary(config), [config])

	return (
		<div className="space-y-3">
			{/* Preset selector */}
			<div className="space-y-2">
				<Label>Schedule</Label>
				<Select
					value={isCustom || showCustomBuilder ? CUSTOM_PRESET_KEY : presetKey}
					onValueChange={handlePresetChange}
					items={presetItems}
				>
					<SelectTrigger className="w-full">
						<CalendarClockIcon className="size-4 text-muted-foreground" aria-hidden="true" />
						<SelectValue placeholder="Choose a schedule" />
					</SelectTrigger>
					<SelectContent side="bottom" align="start" alignItemWithTrigger={false}>
						{SCHEDULE_PRESETS.map((preset) => (
							<SelectItem key={preset.key} value={preset.key}>
								{preset.label}
							</SelectItem>
						))}
						<SelectSeparator />
						<SelectItem value={CUSTOM_PRESET_KEY}>Custom</SelectItem>
					</SelectContent>
				</Select>
			</div>

			{/* Custom sentence builder */}
			{(showCustomBuilder || isCustom) && (
				<CustomScheduleBuilder
					config={config}
					setMode={setMode}
					setTime={setTime}
					setIntervalValue={setIntervalValue}
					setIntervalUnit={setIntervalUnit}
					toggleWeekday={toggleWeekday}
				/>
			)}

			{/* Live summary + next runs */}
			<SchedulePreview rrule={value} summary={summary} />
		</div>
	)
}

// ============================================================
// Custom schedule builder (natural language sentence style)
// ============================================================

function CustomScheduleBuilder({
	config,
	setMode,
	setTime,
	setIntervalValue,
	setIntervalUnit,
	toggleWeekday,
}: {
	config: ScheduleConfig
	setMode: (mode: ScheduleConfig["mode"]) => void
	setTime: (time: string) => void
	setIntervalValue: (value: number) => void
	setIntervalUnit: (unit: IntervalUnit) => void
	toggleWeekday: (day: Weekday) => void
}) {
	return (
		<div className="space-y-3 rounded-lg border border-border/50 bg-muted/30 p-3">
			{/* Mode toggle */}
			<div className="flex items-center gap-2">
				<span className="text-sm text-muted-foreground">Type</span>
				<div className="flex rounded-md border border-border/50">
					<button
						type="button"
						onClick={() => setMode("daily")}
						className={`rounded-l-md px-3 py-1 text-xs font-medium transition-colors ${
							config.mode === "daily"
								? "bg-accent text-accent-foreground"
								: "text-muted-foreground hover:text-foreground"
						}`}
					>
						Daily
					</button>
					<button
						type="button"
						onClick={() => setMode("interval")}
						className={`rounded-r-md border-l border-border/50 px-3 py-1 text-xs font-medium transition-colors ${
							config.mode === "interval"
								? "bg-accent text-accent-foreground"
								: "text-muted-foreground hover:text-foreground"
						}`}
					>
						Interval
					</button>
				</div>
			</div>

			{/* Sentence builder */}
			{config.mode === "daily" ? (
				<DailySentence config={config} setTime={setTime} toggleWeekday={toggleWeekday} />
			) : (
				<IntervalSentence
					config={config}
					setIntervalValue={setIntervalValue}
					setIntervalUnit={setIntervalUnit}
					toggleWeekday={toggleWeekday}
				/>
			)}
		</div>
	)
}

// --- Daily: "Every [weekday pills] at [time]"

function DailySentence({
	config,
	setTime,
	toggleWeekday,
}: {
	config: ScheduleConfig
	setTime: (time: string) => void
	toggleWeekday: (day: Weekday) => void
}) {
	return (
		<div className="flex flex-wrap items-center gap-x-2 gap-y-2 text-sm">
			<span className="text-muted-foreground">Every</span>
			<WeekdayPills weekdays={config.weekdays} toggleWeekday={toggleWeekday} />
			<span className="text-muted-foreground">at</span>
			<Input
				type="time"
				value={config.time}
				onChange={(e) => setTime(e.target.value)}
				className="h-7 w-[7rem] text-xs"
			/>
		</div>
	)
}

// --- Interval: "Every [N] [minutes|hours] on [weekday pills]"

const INTERVAL_UNIT_ITEMS: Record<string, string> = {
	minutes: "minutes",
	hours: "hours",
}

function IntervalSentence({
	config,
	setIntervalValue,
	setIntervalUnit,
	toggleWeekday,
}: {
	config: ScheduleConfig
	setIntervalValue: (value: number) => void
	setIntervalUnit: (unit: IntervalUnit) => void
	toggleWeekday: (day: Weekday) => void
}) {
	return (
		<div className="space-y-2">
			<div className="flex items-center gap-2 text-sm">
				<span className="text-muted-foreground">Every</span>
				<Input
					type="number"
					min={1}
					step={1}
					value={config.intervalValue}
					onChange={(e) => setIntervalValue(Number.parseInt(e.target.value, 10) || 1)}
					className="h-7 w-16 text-xs"
				/>
				<Select
					value={config.intervalUnit}
					onValueChange={(v) => v && setIntervalUnit(v as IntervalUnit)}
					items={INTERVAL_UNIT_ITEMS}
				>
					<SelectTrigger className="h-7 w-28 text-xs">
						<SelectValue />
					</SelectTrigger>
					<SelectContent side="bottom" align="start" alignItemWithTrigger={false}>
						<SelectItem value="minutes">minutes</SelectItem>
						<SelectItem value="hours">hours</SelectItem>
					</SelectContent>
				</Select>
			</div>
			<div className="flex items-center gap-2 text-sm">
				<span className="text-muted-foreground">on</span>
				<WeekdayPills weekdays={config.weekdays} toggleWeekday={toggleWeekday} />
			</div>
		</div>
	)
}

// ============================================================
// Weekday toggle pills
// ============================================================

function WeekdayPills({
	weekdays,
	toggleWeekday,
}: {
	weekdays: Weekday[]
	toggleWeekday: (day: Weekday) => void
}) {
	return (
		<div className="flex gap-1">
			{ALL_WEEKDAYS.map((day) => {
				const isSelected = weekdays.includes(day)
				return (
					<Tooltip key={day}>
						<TooltipTrigger
							render={
								<Button
									type="button"
									variant={isSelected ? "default" : "outline"}
									size="icon"
									className="size-7 text-[10px] font-medium"
									onClick={() => toggleWeekday(day)}
								/>
							}
						>
							{WEEKDAY_LABELS[day]}
						</TooltipTrigger>
						<TooltipContent>{isSelected ? "Included" : "Excluded"}</TooltipContent>
					</Tooltip>
				)
			})}
		</div>
	)
}

// ============================================================
// Schedule preview (summary + next runs)
// ============================================================

function SchedulePreview({ rrule, summary }: { rrule: string; summary: string }) {
	const [nextRuns, setNextRuns] = useState<Date[]>([])

	useEffect(() => {
		let cancelled = false
		computeNextRuns(rrule, 3).then((dates) => {
			if (!cancelled) setNextRuns(dates)
		})
		return () => {
			cancelled = true
		}
	}, [rrule])

	return (
		<div className="rounded-lg border border-border/50 bg-muted/30 px-3 py-2.5">
			<p className="text-sm font-medium">{summary}</p>
			{nextRuns.length > 0 && (
				<div className="mt-1.5 flex flex-wrap gap-x-1.5 gap-y-0.5 text-xs text-muted-foreground">
					<span>Next:</span>
					{nextRuns.map((date, i) => (
						<span key={date.toISOString()}>
							{formatNextRun(date)}
							{i < nextRuns.length - 1 && <span className="ml-1.5 text-border">·</span>}
						</span>
					))}
				</div>
			)}
		</div>
	)
}
