import { atom } from "jotai"
import { atomFamily } from "jotai-family"

/**
 * Per-session version counter bumped on Native `item.updated` / `item.removed`.
 * Components subscribe so they re-render when that session's transcript changes.
 */
export const streamingVersionFamily = atomFamily((_sessionId: string) => atom(0))

/**
 * @deprecated Use `streamingVersionFamily(sessionId)` instead.
 * Kept temporarily so any transient consumers still compile.
 */
export const streamingVersionAtom = atom(0)
