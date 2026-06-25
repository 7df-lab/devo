import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { WelcomeStep } from "./welcome-step"

describe("WelcomeStep", () => {
	test("presents Devo as a prominent onboarding brand mark", () => {
		const markup = renderToStaticMarkup(<WelcomeStep onContinue={() => {}} />)

		expect({
			hasBrandMark: markup.includes('data-slot="welcome-brand-mark"'),
			hasProminentBrandSize: markup.includes("size-16"),
			hasDevoHeading: markup.includes("<h2") && markup.includes(">Devo</h2>"),
		}).toEqual({
			hasBrandMark: true,
			hasProminentBrandSize: true,
			hasDevoHeading: true,
		})
	})
})
