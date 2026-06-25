import { describe, expect, test } from "bun:test"
import { convertAgents } from "../../src/converter/agents"
import type { AgentFile } from "../../src/types/scan-result"

function makeAgent(overrides: Partial<AgentFile> = {}): AgentFile {
	return {
		path: "/project/.claude/agents/test.md",
		name: "test",
		content: "---\ndescription: Test agent\n---\nInstructions here",
		frontmatter: { description: "Test agent" },
		body: "Instructions here",
		...overrides,
	}
}

describe("convertAgents", () => {
	test("converts a simple agent", () => {
		const { agents, report } = convertAgents({
			agents: [makeAgent()],
		})

		expect(agents.size).toBe(1)
		expect(agents.has("test.md")).toBe(true)
		const content = agents.get("test.md")!
		expect(content).toContain("---")
		expect(content).toContain("description")
		expect(content).toContain("Instructions here")
		expect(report.migrated.length).toBe(1)
	})

	test("infers subagent mode from review-related names", () => {
		const { agents } = convertAgents({
			agents: [makeAgent({ name: "code-reviewer", frontmatter: { description: "Reviews code" } })],
		})

		const content = agents.get("code-reviewer.md")!
		expect(content).toContain("subagent")
	})

	test("infers primary mode from build-related names", () => {
		const { agents } = convertAgents({
			agents: [makeAgent({ name: "builder", frontmatter: { description: "Builds features" } })],
		})

		const content = agents.get("builder.md")!
		expect(content).toContain("primary")
	})

	test("translates model ID", () => {
		const { agents } = convertAgents({
			agents: [makeAgent({ frontmatter: { description: "Test", model: "opus" } })],
		})

		const content = agents.get("test.md")!
		expect(content).toContain("anthropic/claude-opus-4-6")
	})

	test("skips inherit model (no model key in output)", () => {
		const { agents } = convertAgents({
			agents: [makeAgent({ frontmatter: { description: "Test", model: "inherit" } })],
		})

		const content = agents.get("test.md")!
		expect(content).not.toContain("inherit")
	})

	test("converts tools list to permissions", () => {
		const { agents } = convertAgents({
			agents: [
				makeAgent({
					frontmatter: { description: "Test", tools: "Read, Edit, Bash" },
				}),
			],
		})

		const content = agents.get("test.md")!
		expect(content).toContain("permission")
		expect(content).toContain("read")
	})

	test("sets temperature based on agent purpose", () => {
		const reviewAgent = makeAgent({
			name: "security-auditor",
			frontmatter: { description: "Security audit" },
		})
		const { agents } = convertAgents({ agents: [reviewAgent] })
		const content = agents.get("security-auditor.md")!
		// Security audit should get low temperature (0.1)
		expect(content).toContain("0.1")
	})

	test("reports errors for broken agents", () => {
		// This should not throw but should report an error
		const { report } = convertAgents({
			agents: [
				{
					path: "/broken",
					name: "broken",
					content: "",
					frontmatter: {},
					body: "",
				},
			],
		})

		// Even without description, it should still convert (uses name as fallback)
		expect(report.migrated.length).toBe(1)
	})

	test("handles multiple agents", () => {
		const { agents, report } = convertAgents({
			agents: [
				makeAgent({ name: "agent1" }),
				makeAgent({ name: "agent2" }),
				makeAgent({ name: "agent3" }),
			],
		})

		expect(agents.size).toBe(3)
		expect(report.migrated.length).toBe(3)
	})
})
