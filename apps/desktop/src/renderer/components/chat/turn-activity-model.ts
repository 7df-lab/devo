import { nativeItemType } from "@devo-ai/sdk/v2/client"
import type { ChatMessageEntry } from "../../hooks/use-session-chat"
import { formatNativeToolTitle } from "../../lib/tool-name"

export type TurnSegment =
	| { kind: "activity"; id: string; entries: ChatMessageEntry[] }
	| { kind: "message"; id: string; entry: ChatMessageEntry }

export interface ActivityRow {
	entry: ChatMessageEntry
	result?: ChatMessageEntry
}

const ACTIVITY_TYPES = new Set([
	"reasoning", "toolCall", "toolResult", "commandExecution", "fileChange", "hostedToolCall",
])

/** One disclosure owns all turn activity; visible messages retain their order. */
export function groupTurnSegments(entries: ChatMessageEntry[]): TurnSegment[] {
	const segments: TurnSegment[] = []
	let activity: Extract<TurnSegment, { kind: "activity" }> | undefined
	for (const entry of entries) {
		const type = nativeItemType(entry.info)
		// A normal fresh-kernel notice isn't an actionable warning for the user.
		if (type === "warning" && entry.info.item.code === "ipythonKernelStateFresh") continue
		if (ACTIVITY_TYPES.has(type)) {
			if (activity) activity.entries.push(entry)
			else {
				activity = { kind: "activity", id: entry.info.id, entries: [entry] }
				segments.push(activity)
			}
		} else {
			segments.push({ kind: "message", id: entry.info.id, entry })
		}
	}
	return segments
}

/** Pair a tool's source and output by Native callId, preserving orphan results. */
export function buildActivityRows(entries: ChatMessageEntry[]): ActivityRow[] {
	const results = new Map<string, ChatMessageEntry>()
	const calls = new Set<string>()
	for (const entry of entries) {
		const callId = entry.info.item.callId
		if (typeof callId !== "string" || !callId) continue
		const type = nativeItemType(entry.info)
		if (type === "toolCall") calls.add(callId)
		if (type === "toolResult") results.set(callId, entry)
	}
	return entries.flatMap((entry) => {
		const type = nativeItemType(entry.info)
		const callId = entry.info.item.callId
		if (type === "toolResult" && typeof callId === "string" && calls.has(callId)) return []
		const result = type === "toolCall" && typeof callId === "string" ? results.get(callId) : undefined
		return [{ entry, ...(result ? { result } : {}) }]
	})
}

export function activityRowTitle(row: ActivityRow): string {
	const type = nativeItemType(row.entry.info)
	if (type === "reasoning") return "Thought"
	if (type === "toolResult") return "Output"
	if (type === "commandExecution") return "Terminal"
	if (type === "fileChange") return "File changes"
	return formatNativeToolTitle(String(row.entry.info.item.toolName ?? type))
}

/** Successful completion is implicit; exceptional and live states remain visible. */
export function activityRowStatus(row: ActivityRow): string | undefined {
	const item = row.result?.info ?? row.entry.info
	if (item.state === "interrupted") return "Stopped"
	// The current Native interrupted-tool result is completed with this error
	// payload; treat a deliberate stop separately from an execution failure.
	if (item.item.isError === true && typeof item.item.output === "string" &&
		/^tool execution was interrupted(?: after \d+ms)?$/.test(item.item.output)) return "Stopped"
	if (item.state === "failed" || item.item.isError === true) return "Failed"
	if (item.state === "lost") return "Connection lost"
	if (item.state === "waiting") return "Waiting"
	if (item.state === "running") return "Running"
	return undefined
}

export function activitySummary(entries: ChatMessageEntry[], working: boolean): string {
	const rows = buildActivityRows(entries)
	if (working) {
		const active = [...rows].reverse().find((row) => {
			const status = activityRowStatus(row)
			return status === "Running" || status === "Waiting"
		})
		if (!active) return "Working…"
		if (activityRowStatus(active) === "Waiting") return `Waiting for ${activityRowTitle(active)}…`
		return nativeItemType(active.entry.info) === "reasoning"
			? "Thinking…"
			: `Running ${activityRowTitle(active)}…`
	}
	const issue = rows.find((row) => ["Failed", "Stopped", "Connection lost"].includes(activityRowStatus(row) ?? ""))
	if (issue) return `${activityRowTitle(issue)} · ${activityRowStatus(issue)}`

	const tools = [...new Set(rows.filter((row) => nativeItemType(row.entry.info) !== "reasoning").map(activityRowTitle))]
	const starts = entries.map((entry) => Date.parse(entry.info.createdAt)).filter(Number.isFinite)
	const ends = entries.map((entry) => Date.parse(entry.info.updatedAt)).filter(Number.isFinite)
	const seconds = starts.length && ends.length
		? Math.max(0, Math.ceil((Math.max(...ends) - Math.min(...starts)) / 1000))
		: 0
	const duration = seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`
	const label = tools.length ? "Worked" : "Thought"
	const summary = seconds ? `${label} for ${duration}` : `${label} briefly`
	return summary
}
