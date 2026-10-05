import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")

describe("narrow composer layout", () => {
	test("allows composer controls to wrap before they collide", () => {
		expect(chatViewSource).toContain('<PromptInputFooter className="flex-wrap">')
	})
})
