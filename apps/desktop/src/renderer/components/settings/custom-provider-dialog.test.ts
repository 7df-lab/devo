import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const dialogSource = readFileSync(new URL("./custom-provider-dialog.tsx", import.meta.url), "utf8")

describe("CustomProviderDialog narrow layout", () => {
	test("keeps the actions visible while provider fields scroll", () => {
		expect(dialogSource).toContain(
			'<DialogContent className="sm:max-w-2xl h-[85vh] max-h-[85vh] flex flex-col gap-4 overflow-hidden">',
		)
		expect(dialogSource).toContain(
			'<div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto py-2 pr-1">',
		)
		const footerStart = dialogSource.indexOf(
			'<DialogFooter className="shrink-0 border-t border-border/60 pt-3">',
		)
		const footerEnd = dialogSource.indexOf("</DialogFooter>", footerStart)
		const footer = dialogSource.slice(footerStart, footerEnd)
		expect(footer).toContain("Cancel")
		expect(footer).toContain("Add Provider")
	})
})
