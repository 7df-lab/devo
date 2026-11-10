import { expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { sessionUsageFromProtocol } from "@devo-ai/sdk/v2/client"
import { SessionUsageDetails } from "./session-usage-details"

test("visible session panel shows separate cumulative input, output, caches and total", () => {
	const html = renderToStaticMarkup(<SessionUsageDetails usage={sessionUsageFromProtocol({ totalTokens: 350, inputTokens: 300, outputTokens: 50, reasoningTokens: 10, cacheReadInputTokens: 100, cacheCreationInputTokens: 40, callCount: 2, meteredCallCount: 2, unreportedFields: [] })} />)
	for (const text of ["Cumulative token usage", "Input", "Output", "Cache read", "Cache write", "Total", ">300<", ">50<", ">100<", ">40<", ">350<", "2 requests metered.", "Cache tokens are included in input."]) expect(html).toContain(text)
})

test("unreported fields and partial coverage are visible", () => {
	const unknown = renderToStaticMarkup(<SessionUsageDetails usage={sessionUsageFromProtocol({ totalTokens: 0, callCount: 1, meteredCallCount: 0 })} />)
	expect(unknown).toContain("Unknown")
	expect(unknown).toContain("The provider has not reported token usage.")
	expect(unknown).not.toContain(">0<")
	const partial = renderToStaticMarkup(<SessionUsageDetails usage={sessionUsageFromProtocol({ totalTokens: 120, callCount: 2, meteredCallCount: 1 })} />)
	expect(partial).toContain("Reported tokens only: 1 of 2 requests metered.")
})
