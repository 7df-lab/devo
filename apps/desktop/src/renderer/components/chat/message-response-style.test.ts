import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const messageSource = readFileSync(
	new URL("../../../../packages/ui/src/components/ai-elements/message.tsx", import.meta.url),
	"utf8",
)
const rendererCssSource = readFileSync(new URL("../../index.css", import.meta.url), "utf8")

describe("MessageResponse markdown surfaces", () => {
	test("uses desktop dark theme surfaces for streamdown markdown cells", () => {
		expect({
			responseClass: messageSource.includes("devo-message-response"),
			codeBlockSurface: rendererCssSource.includes('[data-streamdown="code-block"]'),
			codeBlockBodySurface: rendererCssSource.includes('[data-streamdown="code-block-body"]'),
			tableHeaderSurface: rendererCssSource.includes('[data-streamdown="table-header"]'),
		}).toEqual({
			responseClass: true,
			codeBlockSurface: true,
			codeBlockBodySurface: true,
			tableHeaderSurface: true,
		})
	})

	test("keeps transcript markdown headings visually compact", () => {
		expect({
			requirementComment: messageSource.includes(
				"transcript Markdown headings should look like bold body text",
			),
			headingComponents: messageSource.includes("const transcriptMarkdownComponents"),
			headingStyle: messageSource.includes(
				"my-2 border-0 pb-0 text-sm font-semibold leading-6 text-foreground",
			),
		}).toEqual({
			requirementComment: true,
			headingComponents: true,
			headingStyle: true,
		})
	})
})
