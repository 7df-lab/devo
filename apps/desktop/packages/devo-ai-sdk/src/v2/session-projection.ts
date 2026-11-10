import type { Session } from "./client"

export function objectRecord(value: unknown): Record<string, unknown> | undefined {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : undefined
}

function stringOrUndefined(value: unknown): string | undefined {
	return typeof value === "string" && value.length > 0 ? value : undefined
}

export function parseTimestampMs(value: unknown): number | undefined {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value !== "string") return undefined
	const parsed = Date.parse(value)
	return Number.isFinite(parsed) ? parsed : undefined
}

function parseTitleState(value: unknown): string | undefined {
	if (typeof value === "string") return value
	if (value && typeof value === "object") {
		const keys = Object.keys(value as Record<string, unknown>)
		if (keys.length === 1) return keys[0]
	}
	return undefined
}

/** Project Native metadata without writing caches or changing session status. */
export function projectNativeSession(
	info: Record<string, unknown>,
	existing: Session | undefined,
	directory: string,
): Session {
	const id = String(info.id ?? "")
	if (!id) throw new Error("Native session is missing id")
	const usage = objectRecord(info.usage)
	const total = objectRecord(usage?.total)
	const created = parseTimestampMs(info.createdAt) ?? existing?.time.created ?? Date.now()
	const wireActivity = parseTimestampMs(info.lastActivityAt)
	const existingActivity = Math.max(existing?.time.lastActivity ?? 0, existing?.time.updated ?? 0)
	// Never let resume/snapshot lower a known activity timestamp — that alone
	// can reshuffle the sidebar. Prefer the newer of wire vs in-memory.
	const updated =
		wireActivity != null
			? Math.max(wireActivity, existingActivity)
			: existingActivity > 0
				? existingActivity
				: created
	const parent = objectRecord(info.parent)
	const forkFromId =
		typeof info.forkFromId === "string"
			? info.forkFromId
			: typeof info.fork_from_id === "string"
				? info.fork_from_id
				: existing?.forkFromId
	const atTurnId =
		typeof info.atTurnId === "string"
			? info.atTurnId
			: typeof info.fork_at_turn_id === "string"
				? info.fork_at_turn_id
				: existing?.atTurnId
	const titleState = parseTitleState(info.titleState ?? info.title_state)
	// Persisted per-session turn settings (model / reasoning effort / mode).
	// The server restores these on resume; without them the Desktop composer
	// cannot re-seed per-session selections after a restart and every
	// session falls back to the project default.
	const wireModel = objectRecord(info.model)
	const wireSettings = objectRecord(info.settings)
	const sessionModel = wireModel
		? {
				provider:
					typeof wireModel.provider === "string" ? wireModel.provider : existing?.model?.provider,
				model: typeof wireModel.model === "string" ? wireModel.model : existing?.model?.model,
				reasoningEffort:
					stringOrUndefined(wireModel.reasoningEffort ?? wireModel.reasoning_effort) ??
					existing?.model?.reasoningEffort,
			}
		: existing?.model
	const sessionSettings = wireSettings
		? {
				mode: stringOrUndefined(wireSettings.mode) ?? existing?.settings?.mode,
				reasoningEffort:
					stringOrUndefined(wireSettings.reasoningEffort ?? wireSettings.reasoning_effort) ??
					existing?.settings?.reasoningEffort,
				permissionProfile:
					stringOrUndefined(wireSettings.permissionProfile ?? wireSettings.permission_profile) ??
					existing?.settings?.permissionProfile,
			}
		: existing?.settings
	const session: Session = {
		id,
		title: typeof info.title === "string" ? info.title : existing?.title,
		titleState: titleState ?? existing?.titleState ?? "Unset",
		parentId: typeof parent?.sessionId === "string" ? parent.sessionId : existing?.parentId,
		forkFromId,
		atTurnId,
		time: { created, updated, lastActivity: updated },
		directory: String(info.cwd ?? existing?.directory ?? directory),
		model: sessionModel,
		settings: sessionSettings,
		totalInputTokens: Number(total?.inputTokens ?? existing?.totalInputTokens ?? 0),
		totalOutputTokens: Number(total?.outputTokens ?? existing?.totalOutputTokens ?? 0),
		totalTokens: Number(total?.totalTokens ?? existing?.totalTokens ?? 0),
		totalCacheCreationTokens: Number(
			total?.cacheCreationInputTokens ?? existing?.totalCacheCreationTokens ?? 0,
		),
		totalCacheReadTokens: Number(
			total?.cacheReadInputTokens ?? existing?.totalCacheReadTokens ?? 0,
		),
		promptTokenEstimate: Number(total?.inputTokens ?? existing?.promptTokenEstimate ?? 0),
		lastQueryTotalTokens: existing?.lastQueryTotalTokens ?? 0,
	}
	return session
}
