import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const sideRailSource = readFileSync(new URL("./review-side-rail.tsx", import.meta.url), "utf8")
const resizeHandleSource = readFileSync(new URL("./right-panel-resize-handle.tsx", import.meta.url), "utf8")

describe("ReviewSideRail responsive accessibility", () => {
	test("measures the split container before clamping the panel width", () => {
		expect(sideRailSource).toContain("railRef.current?.parentElement")
		expect(sideRailSource).toContain("observer.observe(container)")
		expect(sideRailSource).toContain("availableWidth: splitWidth")
	})

	test("keeps closed panel controls out of the focus and accessibility trees", () => {
		expect(sideRailSource).toContain("aria-hidden={!open}")
		expect(sideRailSource).toContain("inert={!open}")
	})

	test("reports the panel's actual available width to assistive technology", () => {
		expect(resizeHandleSource).toContain("aria-valuemax={clampReviewPanelWidth(REVIEW_PANEL_MAX_WIDTH_PX, { availableWidth })}")
	})
})
