import { describe, expect, test } from "bun:test"
import { scheduleOccurrences } from "./automation-schedule"

const daily = "FREQ=DAILY;BYHOUR=9;BYMINUTE=0;BYSECOND=0"
const iso = (dates: Date[]) => dates.map((date) => date.toISOString())

describe("automation calendar timezone", () => {
	test("uses the configured timezone, including fractional offsets and date rollover", async () => {
		const after = new Date("2026-11-08T00:00:00Z")
		expect(iso(await scheduleOccurrences(daily, "Asia/Shanghai", { after, count: 2 }))).toEqual([
			"2026-11-08T01:00:00.000Z",
			"2026-11-09T01:00:00.000Z",
		])
		expect(iso(await scheduleOccurrences(daily, "Asia/Kathmandu", { after, count: 1 }))).toEqual([
			"2026-11-08T03:15:00.000Z",
		])
		expect(iso(await scheduleOccurrences(daily, "Pacific/Auckland", { after, count: 1 }))).toEqual([
			"2026-11-08T20:00:00.000Z",
		])
	})

	test("keeps 9am wall time across daylight saving changes", async () => {
		expect(
			iso(
				await scheduleOccurrences(daily, "America/New_York", {
					after: new Date("2026-03-07T00:00:00Z"),
					count: 3,
				}),
			),
		).toEqual(["2026-03-07T14:00:00.000Z", "2026-03-08T13:00:00.000Z", "2026-03-09T13:00:00.000Z"])
	})

	test("skips nonexistent spring-forward times", async () => {
		expect(
			iso(
				await scheduleOccurrences(
					"FREQ=DAILY;BYHOUR=2;BYMINUTE=30;BYSECOND=0",
					"America/New_York",
					{
						after: new Date("2026-03-08T00:00:00Z"),
						count: 1,
					},
				),
			),
		).toEqual(["2026-03-09T06:30:00.000Z"])
	})

	test("fires repeated fall-back times once at the earlier instant", async () => {
		const rule = "FREQ=DAILY;BYHOUR=1;BYMINUTE=30;BYSECOND=0"
		const zone = "America/New_York"
		expect(
			iso(
				await scheduleOccurrences(rule, zone, {
					after: new Date("2026-11-01T00:00:00Z"),
					count: 2,
				}),
			),
		).toEqual(["2026-11-01T05:30:00.000Z", "2026-11-02T06:30:00.000Z"])
		expect(
			iso(
				await scheduleOccurrences(rule, zone, {
					after: new Date("2026-11-01T06:15:00Z"),
					count: 1,
				}),
			),
		).toEqual(["2026-11-02T06:30:00.000Z"])
	})

	test("handles half-hour DST transitions", async () => {
		expect(
			iso(
				await scheduleOccurrences(
					"FREQ=DAILY;BYHOUR=2;BYMINUTE=15;BYSECOND=0",
					"Australia/Lord_Howe",
					{
						after: new Date("2026-10-03T16:00:00Z"),
						count: 1,
					},
				),
			),
		).toEqual(["2026-10-04T15:15:00.000Z"])
	})

	test("honors UTC UNTIL and returns no exhausted occurrences", async () => {
		expect(
			iso(
				await scheduleOccurrences(`${daily};UNTIL=20261109T010000Z`, "Asia/Shanghai", {
					after: new Date("2026-11-08T00:00:00Z"),
					count: 3,
				}),
			),
		).toEqual(["2026-11-08T01:00:00.000Z", "2026-11-09T01:00:00.000Z"])
	})

	test("keeps an interval anchor across recalculations", async () => {
		expect(
			iso(
				await scheduleOccurrences("FREQ=MINUTELY;INTERVAL=30", "Asia/Shanghai", {
					startAt: new Date("2026-11-08T00:00:00Z"),
					after: new Date("2026-11-08T00:32:00Z"),
					count: 1,
				}),
			),
		).toEqual(["2026-11-08T01:00:00.000Z"])
	})

	test("rejects invalid zones, rules and conflicting embedded TZID", async () => {
		await expect(scheduleOccurrences(daily, "Invalid/Zone")).rejects.toThrow()
		await expect(scheduleOccurrences("invalid", "UTC")).rejects.toThrow()
		await expect(
			scheduleOccurrences("DTSTART;TZID=Asia/Tokyo:20261108T090000\nRRULE:FREQ=DAILY", "UTC"),
		).rejects.toThrow()
	})
})
