import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ToolCard } from "./tool-card"

describe("ToolCard", () => {
	test("renders without a left-edge category accent", () => {
		const markup = renderToStaticMarkup(
			<ToolCard icon={<span data-testid="icon" />} title="Shell" category="run" />,
		)

		expect({
			hasTitle: markup.includes(">Shell<"),
			hasLeftBorderAccent: markup.includes("border-l-"),
		}).toEqual({
			hasTitle: true,
			hasLeftBorderAccent: false,
		})
	})

	test("does not render collapsed lazy content", () => {
		let renderCount = 0
		const markup = renderToStaticMarkup(
			<ToolCard
				icon={<span data-testid="icon" />}
				title="Shell"
				hasContent
				renderContent={() => {
					renderCount += 1
					return <span>expensive output</span>
				}}
			/>,
		)

		expect({ renderCount, hasContent: markup.includes("expensive output") }).toEqual({
			renderCount: 0,
			hasContent: false,
		})
	})

	test("renders lazy content when forced open", () => {
		let renderCount = 0
		const markup = renderToStaticMarkup(
			<ToolCard
				icon={<span data-testid="icon" />}
				title="Shell"
				hasContent
				forceOpen
				renderContent={() => {
					renderCount += 1
					return <span>error output</span>
				}}
			/>,
		)

		expect({ renderCount, hasContent: markup.includes("error output") }).toEqual({
			renderCount: 1,
			hasContent: true,
		})
	})
})
