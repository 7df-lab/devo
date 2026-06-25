import { atom } from "jotai"
import { atomFamily, atomWithStorage } from "jotai/utils"
import type { FileDiff } from "../lib/types"

export const commandPaletteOpenAtom = atom(false)

/**
 * The session ID currently being viewed in the main content area.
 * Set by the router/session view when the user navigates to a session.
 * Used by metrics atoms to skip expensive recomputation for background sessions.
 */
export const viewedSessionIdAtom = atom<string | null>(null)

// ============================================================
// Review Panel State
// ============================================================

/** Whether the review panel is open (resets to closed on app start) */
export const reviewPanelOpenAtom = atom(false)

/**
 * File path to highlight in the review panel.
 * Set by external components (e.g. edit tool card "View diff" button).
 * The ReviewPanel subscribes to this and syncs it with its local selectedFile state.
 * Cleared automatically after the panel consumes it.
 */
export const reviewPanelSelectedFileAtom = atom<string | null>(null)

/**
 * Action atom: opens the review panel and jumps to a specific file.
 * Usage: `const viewDiff = useSetAtom(viewFileInDiffPanelAtom)`
 *        `viewDiff("src/foo.ts")`
 */
export const viewFileInDiffPanelAtom = atom(null, (_get, set, filePath: string) => {
	set(reviewPanelOpenAtom, true)
	set(reviewPanelSelectedFileAtom, filePath)
})

/** Diff display style preference */
export type DiffStyle = "unified" | "split"

/** Review panel user preferences (persisted to localStorage) */
export interface ReviewPanelSettings {
	/** Diff rendering style: unified (single column) or split (side-by-side) */
	diffStyle: DiffStyle
	/** Whether the review panel is expanded to full width */
	expanded: boolean
}

export const reviewPanelSettingsAtom = atomWithStorage<ReviewPanelSettings>(
	"devo:review-panel-settings",
	{ diffStyle: "unified", expanded: false },
)

/** Per-session diff data from the Devo API */
export const sessionDiffFamily = atomFamily((_sessionId: string) => atom<FileDiff[]>([]))

/** Write-only atom to update session diff data */
export const setSessionDiffAtom = atom(
	null,
	(_get, set, args: { sessionId: string; diffs: FileDiff[] }) => {
		set(sessionDiffFamily(args.sessionId), args.diffs)
	},
)

/** Per-session diff filter: null = all session changes, string = specific messageID */
export const diffFilterFamily = atomFamily((_sessionId: string) => atom<string | null>(null))

/** Computed total stats for a session's diffs (all files, including generated) */
export const sessionDiffStatsFamily = atomFamily((sessionId: string) =>
	atom((get) => {
		const diffs = get(sessionDiffFamily(sessionId))
		let additions = 0
		let deletions = 0
		for (const diff of diffs) {
			additions += diff.additions
			deletions += diff.deletions
		}
		return { additions, deletions, fileCount: diffs.length }
	}),
)
