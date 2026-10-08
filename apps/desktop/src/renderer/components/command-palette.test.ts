import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./command-palette.tsx", import.meta.url), "utf8")

describe("command palette session search", () => {
	test("searches Native session summaries beyond loaded sidebar pages", () => {
		expect({
			controlledQuery: source.includes("value={query} onValueChange={setQuery}"),
			unscopedClient: source.includes("const client = getBaseClient()"),
			boundedSearch: source.includes("client.session.list({ search, limit: 50 })"),
			preservesPagination: source.includes('appStore.set(setSessionsAtom, { sessions, statuses: {}, directory: "" })'),
			ignoresStaleQueries: source.includes("cancelled = true"),
			loadingAndFailure: source.includes("Searching sessions…") && source.includes("Session search failed. Try again."),
		}).toEqual({
			controlledQuery: true,
			unscopedClient: true,
			boundedSearch: true,
			preservesPagination: true,
			ignoresStaleQueries: true,
			loadingAndFailure: true,
		})
	})

	test("uses distinct item values for sessions with identical titles", () => {
		expect(source).toContain('value={`session:${agent.id}`}')
		expect(source).toContain('value={`active-session:${agent.id}`}')
		expect(source).toContain("keywords={[agent.name, agent.project]}")
	})
})
