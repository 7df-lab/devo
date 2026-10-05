import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const terminalPanelSource = readFileSync(new URL("./desktop-terminal-panel.tsx", import.meta.url), "utf8")

describe("DesktopTerminalPanel collapsed state", () => {
	test("removes hidden terminal controls from keyboard focus", () => {
		expect(terminalPanelSource).toContain("aria-hidden={!open}")
		expect(terminalPanelSource).toContain("inert={!open}")
	})
})
