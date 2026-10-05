import { cn } from "@devo/ui/lib/utils"
import { useAtom } from "jotai"
import { memo, useCallback, useLayoutEffect, useRef, useState } from "react"
import { reviewPanelOpenAtom, reviewPanelSettingsAtom } from "../../atoms/ui"
import { clampReviewPanelWidth, REVIEW_PANEL_DEFAULT_WIDTH_PX } from "../../lib/review-panel-width"
import { RightPanel } from "../right-panel"
import { RightPanelResizeHandle } from "./right-panel-resize-handle"

interface ReviewSideRailProps {
	sessionId: string
	directory: string
}

/**
 * Right session rail with persisted pixel width and a left-edge resize handle
 * (same interaction model as the app sidebar).
 *
 * Expanded mode uses flex growth instead of `width: 100%` so restore can
 * reliably return to a pixel width (%-to-px width transitions get stuck).
 */
export const ReviewSideRail = memo(function ReviewSideRail({
	sessionId,
	directory,
}: ReviewSideRailProps) {
	const [open] = useAtom(reviewPanelOpenAtom)
	const [settings, setSettings] = useAtom(reviewPanelSettingsAtom)
	const [resizing, setResizing] = useState(false)
	const railRef = useRef<HTMLDivElement>(null)
	const [availableWidth, setAvailableWidth] = useState(0)

	useLayoutEffect(() => {
		const container = railRef.current?.parentElement
		if (!container) return

		const updateAvailableWidth = () => setAvailableWidth(container.clientWidth)
		updateAvailableWidth()
		if (typeof ResizeObserver === "undefined") {
			window.addEventListener("resize", updateAvailableWidth)
			return () => window.removeEventListener("resize", updateAvailableWidth)
		}

		const observer = new ResizeObserver(updateAvailableWidth)
		observer.observe(container)
		return () => observer.disconnect()
	}, [])

	const splitWidth =
		availableWidth > 0
			? availableWidth
			: typeof window !== "undefined"
				? window.innerWidth
				: undefined
	const widthOptions = splitWidth === undefined ? undefined : { availableWidth: splitWidth }
	const widthPx = clampReviewPanelWidth(
		settings.widthPx ?? REVIEW_PANEL_DEFAULT_WIDTH_PX,
		widthOptions,
	)
	const expanded = Boolean(settings.expanded)

	const handleWidthChange = useCallback(
		(next: number) => {
			setSettings((prev) => ({
				...prev,
				expanded: false,
				widthPx: clampReviewPanelWidth(
					next,
					splitWidth === undefined ? undefined : { availableWidth: splitWidth },
				),
			}))
		},
		[setSettings, splitWidth],
	)

	return (
		<div
			ref={railRef}
			aria-hidden={!open}
			inert={!open}
			className={cn(
				"relative min-w-0 overflow-hidden",
				open && "border-l border-border/70",
				!open && "w-0 shrink-0",
				open && expanded && "min-w-0 flex-1",
				open && !expanded && "shrink-0",
			)}
			data-resizing={resizing ? "true" : undefined}
			style={
				!open
					? { width: 0 }
					: expanded
						? undefined
						: { width: widthPx }
			}
		>
			{open && !expanded ? (
				<RightPanelResizeHandle
					width={widthPx}
					availableWidth={splitWidth ?? REVIEW_PANEL_DEFAULT_WIDTH_PX}
					onWidthChange={handleWidthChange}
					onResizingChange={setResizing}
				/>
			) : null}
			{sessionId ? (
				<div className="h-full w-full min-w-0" style={{ minWidth: open && !expanded ? widthPx : undefined }}>
					<RightPanel sessionId={sessionId} directory={directory} />
				</div>
			) : null}
		</div>
	)
})
