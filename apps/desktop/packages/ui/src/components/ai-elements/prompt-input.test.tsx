import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { PromptInput, PromptInputProvider, PromptInputTextarea } from "./prompt-input"

function textareaClassList(html: string): string[] {
	const match = html.match(/<textarea[^>]*class="([^"]+)"/)
	if (!match) return []
	return match[1].split(/\s+/)
}

function inputGroupClassList(html: string): string[] {
	const match = html.match(/<div[^>]*data-slot="input-group"[^>]*class="([^"]+)"/)
	if (!match) return []
	return match[1].split(/\s+/)
}

describe("PromptInputTextarea", () => {
	test("renders prompt hints in muted gray separate from entered text", () => {
		const html = renderToStaticMarkup(
			<PromptInputProvider>
				<PromptInput onSubmit={() => undefined}>
					<PromptInputTextarea placeholder="What would you like to do?" />
				</PromptInput>
			</PromptInputProvider>,
		)

		const classes = textareaClassList(html)

		expect(classes).toContain("text-foreground")
		expect(classes).toContain("placeholder:text-muted-foreground/50")
		expect(classes).not.toContain("placeholder:text-muted-foreground")
	})

	test("keeps composer focus edges neutral instead of blue", () => {
		const html = renderToStaticMarkup(
			<PromptInputProvider>
				<PromptInput onSubmit={() => undefined}>
					<PromptInputTextarea placeholder="What would you like to do?" />
				</PromptInput>
			</PromptInputProvider>,
		)

		const classes = inputGroupClassList(html)

		expect(classes).toContain(
			"has-[[data-slot=input-group-control]:focus-visible]:border-input",
		)
		expect(classes).toContain("has-[[data-slot=input-group-control]:focus-visible]:ring-0")
		expect(classes).not.toContain(
			"has-[[data-slot=input-group-control]:focus-visible]:border-ring",
		)
		expect(classes).not.toContain(
			"has-[[data-slot=input-group-control]:focus-visible]:ring-ring/50",
		)
		expect(classes).not.toContain("has-[[data-slot=input-group-control]:focus-visible]:ring-3")
	})
})
