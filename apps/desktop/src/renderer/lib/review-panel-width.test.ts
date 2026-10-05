import { describe, expect, test } from "bun:test"
import { clampReviewPanelWidth } from "./review-panel-width"

describe("clampReviewPanelWidth", () => {
	test("sizes the panel from its available split-container width", () => {
		expect(clampReviewPanelWidth(480, { availableWidth: 620 })).toBe(280)
		expect(clampReviewPanelWidth(480, { availableWidth: 920 })).toBe(480)
	})

	test("keeps persisted widths within the configured bounds", () => {
		expect(clampReviewPanelWidth(1_500, { availableWidth: 1_320 })).toBe(900)
		expect(clampReviewPanelWidth(100, { availableWidth: 920 })).toBe(280)
	})
})
