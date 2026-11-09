import type { Session } from "./generated/native"

/** Native session/list parameters. The protocol generator exports Session,
 * but not the list RPC types; runtime validation still uses the Native schema. */
export interface SessionRosterParams {
	cwds?: string[]
	search?: string
	cursor?: string
	limit?: number
	includeChildren?: boolean
	/** Direct subagents only; implies includeChildren on supported runtimes. */
	parentSessionId?: string
}

export interface SessionRosterPage {
	data: Session[]
	nextCursor?: string | null
}
