import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { Terminal } from "./terminal"

describe("Terminal", () => {
	test("uses theme tokens instead of a hard-coded dark surface", () => {
		const markup = renderToStaticMarkup(<Terminal output="hello" />)

		expect({
			hasThemeBackground: markup.includes("bg-background"),
			hasThemeBorder: markup.includes("border-border"),
			hasThemeText: markup.includes("text-foreground"),
			hasMutedHeader: markup.includes("bg-muted/40"),
			hasMutedControls: markup.includes("text-muted-foreground"),
			hasFixedDarkSurface: markup.includes("bg-zinc-950"),
			hasFixedLightText: markup.includes("text-zinc-100"),
		}).toEqual({
			hasThemeBackground: true,
			hasThemeBorder: true,
			hasThemeText: true,
			hasMutedHeader: true,
			hasMutedControls: true,
			hasFixedDarkSurface: false,
			hasFixedLightText: false,
		})
	})

	test("uses the foreground token for the streaming cursor", () => {
		const markup = renderToStaticMarkup(<Terminal output="hello" isStreaming />)

		expect({
			hasThemeCursor: markup.includes("bg-foreground"),
			hasFixedCursor: markup.includes("bg-zinc-100"),
		}).toEqual({
			hasThemeCursor: true,
			hasFixedCursor: false,
		})
	})
})
