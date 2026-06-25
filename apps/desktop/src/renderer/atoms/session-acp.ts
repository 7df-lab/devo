import { atom } from "jotai"
import { atomFamily } from "jotai-family"

export interface SessionAcpState {
	commands: unknown[]
	configOptions: unknown[]
	modeID?: string
	usage?: {
		used: unknown
		size: unknown
		cost?: unknown
	}
}

export const sessionAcpFamily = atomFamily((_sessionId: string) =>
	atom<SessionAcpState>({
		commands: [],
		configOptions: [],
	}),
)
