import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"

const source = readFileSync(new URL("./command-palette.tsx", import.meta.url), "utf8")

describe("command palette session search", () => {
	test("uses distinct item values for sessions with identical titles", () => {
		expect(source).toContain("value={`session:${agent.id}`}")
		expect(source).toContain("value={`active-session:${agent.id}`}")
		expect(source).toContain("keywords={[agent.name, agent.project]}")
	})
})
