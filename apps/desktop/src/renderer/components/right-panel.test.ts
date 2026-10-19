import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const rightPanelSource = readFileSync(new URL("./right-panel.tsx", import.meta.url), "utf8")

describe("RightPanel keyboard controls", () => {
	test("reveals inactive tab close actions on keyboard focus", () => {
		expect(rightPanelSource).toContain("group-focus-within:opacity-100")
		expect(rightPanelSource).toContain("focus-visible:opacity-100")
		expect(rightPanelSource).toContain("focus-visible:ring-2")
	})

	test("names icon-only panel size and visibility controls", () => {
		expect(rightPanelSource).toContain(
			'aria-label={settings.expanded ? "Restore panel size" : "Expand to full width"}',
		)
		expect(rightPanelSource).toContain('aria-label="Hide side panel"')
	})
})
