import { atom } from "jotai"
import { atomFamily } from "jotai/utils"
import type { ModelRef } from "../hooks/use-devo-data"
import type { PersistedModelRef } from "./preferences"

export interface SessionComposerState {
	model: ModelRef | null
	variant?: string
	agent: string | null
	/** Set when the user explicitly changes composer settings for this session. */
	hasUserOverride: boolean
}

const EMPTY_COMPOSER_STATE: SessionComposerState = {
	model: null,
	variant: undefined,
	agent: null,
	hasUserOverride: false,
}

export { EMPTY_COMPOSER_STATE }

export const sessionComposerFamily = atomFamily((_sessionId: string) =>
	atom<SessionComposerState>(EMPTY_COMPOSER_STATE),
)

export function composerFromPersistedModel(
	stored: PersistedModelRef | undefined,
): SessionComposerState {
	if (!stored?.providerID || !stored?.modelID) {
		return EMPTY_COMPOSER_STATE
	}
	return {
		model: { providerID: stored.providerID, modelID: stored.modelID },
		variant: stored.variant,
		agent: stored.agent ?? null,
		hasUserOverride: false,
	}
}

export interface SessionModelSeed {
	provider?: string
	model?: string
	reasoningEffort?: string
}

/**
 * Builds composer state from the persisted wire-session model settings.
 * `resolveModel` maps the seed to a full ModelRef — preferring the wire
 * provider id (`session/resume` carries a real one) and falling back to a
 * reverse slug lookup across providers (cold `session/list` snapshots may
 * only know `"unknown"`).
 */
export function composerFromSessionModel(
	seed: SessionModelSeed | null | undefined,
	resolveModel: (seed: SessionModelSeed) => ModelRef | null,
): SessionComposerState | null {
	if (!seed?.model) return null
	const model = resolveModel(seed)
	if (!model) return null
	return {
		model,
		variant: seed.reasoningEffort,
		agent: null,
		hasUserOverride: false,
	}
}

/**
 * Hydrate composer from session seed / project default.
 * Native transcripts do not carry per-message composer metadata.
 */
export function hydrateSessionComposerState(
	current: SessionComposerState,
	itemCount: number,
	projectDefault: PersistedModelRef | undefined,
	/** Persisted per-session turn settings from the wire session (server restores them). */
	sessionSeed?: SessionComposerState | null,
): SessionComposerState {
	if (current.hasUserOverride) return current
	if (sessionSeed) return sessionSeed
	if (itemCount > 0) return current
	return composerFromPersistedModel(projectDefault)
}

export const setSessionComposerAtom = atom(
	null,
	(
		_get,
		set,
		args: {
			sessionId: string
			patch: Partial<SessionComposerState>
			userOverride?: boolean
		},
	) => {
		set(sessionComposerFamily(args.sessionId), (current) => ({
			...current,
			...args.patch,
			hasUserOverride: args.userOverride ? true : current.hasUserOverride,
		}))
	},
)
