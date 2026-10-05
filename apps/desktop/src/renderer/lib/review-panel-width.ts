/** Default right Changes panel width (≈40% of a typical desktop window). */
export const REVIEW_PANEL_DEFAULT_WIDTH_PX = 480
export const REVIEW_PANEL_MIN_WIDTH_PX = 280
export const REVIEW_PANEL_MAX_WIDTH_PX = 900

/** Clamp the right review panel width to its available split-container width. */
export function clampReviewPanelWidth(
	width: number,
	options?: { availableWidth?: number; contentMinWidth?: number },
): number {
	const availableWidth = options?.availableWidth ?? Number.POSITIVE_INFINITY
	const contentMinWidth = options?.contentMinWidth ?? 360
	const maxForContainer = Number.isFinite(availableWidth)
		? Math.max(REVIEW_PANEL_MIN_WIDTH_PX, availableWidth - contentMinWidth)
		: REVIEW_PANEL_MAX_WIDTH_PX
	const max = Math.min(REVIEW_PANEL_MAX_WIDTH_PX, maxForContainer)
	if (!Number.isFinite(width)) return REVIEW_PANEL_DEFAULT_WIDTH_PX
	return Math.min(max, Math.max(REVIEW_PANEL_MIN_WIDTH_PX, Math.round(width)))
}
