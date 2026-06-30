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
			headingClassWins: messageSource.includes(
				"className,\n\t\t\t\t\"my-2 border-0 pb-0 text-sm font-semibold leading-6 text-foreground\"",
			),
			headingStyle: messageSource.includes(
				"my-2 border-0 pb-0 text-sm font-semibold leading-6 text-foreground",
			),
			markdownRulesStillRender: !messageSource.includes("hr: TranscriptMarkdownRule"),
		}).toEqual({
			requirementComment: true,
			headingComponents: true,
			headingClassWins: true,
			headingStyle: true,
			markdownRulesStillRender: true,
		})
	})

	test("keeps streamdown code block actions in the language header row", () => {
		expect({
			headerPadding: rendererCssSource.includes('[data-streamdown="code-block-header"]'),
			actionsSiblingSelector: rendererCssSource.includes(
				'> div:has(> [data-streamdown="code-block-actions"])',
			),
			actionsAbsolute: rendererCssSource.includes("position: absolute;"),
			actionsStillClickable: rendererCssSource.includes("pointer-events: auto;"),
		}).toEqual({
			headerPadding: true,
			actionsSiblingSelector: true,
			actionsAbsolute: true,
			actionsStillClickable: true,
		})
	})

	test("removes fullscreen from regular markdown table controls only", () => {
		expect({
			controlsConfig: messageSource.includes("const transcriptMarkdownControls"),
			tableFullscreenDisabled: messageSource.includes("fullscreen: false"),
			controlsPassedToStreamdown: messageSource.includes("controls={transcriptMarkdownControls}"),
			tableCopyNotDisabled: !messageSource.includes("copy: false"),
			tableDownloadNotDisabled: !messageSource.includes("download: false"),
		}).toEqual({
			controlsConfig: true,
			tableFullscreenDisabled: true,
			controlsPassedToStreamdown: true,
			tableCopyNotDisabled: true,
			tableDownloadNotDisabled: true,
		})
	})
})
