import type { SessionUsageSummary } from "@devo-ai/sdk/v2/client"
import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { ContextOccupancy } from "../lib/context-occupancy"

export interface SessionNativeState {
	commands: unknown[]
	configOptions: unknown[]
	modeID?: string
	usage?: {
		used: unknown
		size: unknown
		cost?: unknown
	}
	cumulativeUsage?: SessionUsageSummary
	occupancy?: ContextOccupancy
}

export const sessionNativeFamily = atomFamily((_sessionId: string) =>
	atom<SessionNativeState>({
		commands: [],
		configOptions: [],
	}),
)
