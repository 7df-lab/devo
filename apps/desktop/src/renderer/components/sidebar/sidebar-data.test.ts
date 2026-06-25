import { describe, expect, test } from "bun:test"
import type { Agent, SidebarProject } from "../../lib/types"
import { buildSidebarItems, type SidebarPreferences } from "./sidebar-data"

function project(name: string, directory: string, lastActiveAt: number): SidebarProject {
	return {
		id: `${name}-id`,
		slug: `${name}-slug`,
		name,
		directory,
		agentCount: 2,
		lastActiveAt,
		hasActiveAgent: false,
	}
}

function agent(
	id: string,
	projectInfo: SidebarProject,
	createdAt: number,
	lastActiveAt: number,
): Agent {
	return {
		id,
		sessionId: id,
		name: `${id} session`,
		status: "idle",
		environment: "local",
		project: projectInfo.name,
		projectSlug: projectInfo.slug,
		directory: projectInfo.directory,
		projectDirectory: projectInfo.directory,
		branch: "main",
		duration: "1h",
		activities: [],
		permissions: [],
		questions: [],
		createdAt,
		lastActiveAt,
	}
}

describe("sidebar data helpers", () => {
	const alpha = project("alpha", "/repo/alpha", 200)
	const beta = project("beta", "/repo/beta", 400)
	const alphaOlder = agent("alpha-older", alpha, 10, 30)
	const alphaNewer = agent("alpha-newer", alpha, 20, 80)
	const betaOnly = agent("beta-only", beta, 30, 60)

	test("by-project groups preserve project sections and loaded sessions", () => {
		const preferences: SidebarPreferences = { organization: "by-project", sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
			}),
		).toEqual([
			{ type: "project", project: alpha, sessions: [alphaNewer, alphaOlder] },
			{ type: "project", project: beta, sessions: [betaOnly] },
		])
	})

	test("by-project order can remain stable when project activity changes", () => {
		const preferences: SidebarPreferences = { organization: "by-project", sort: "updated" }
		const stableOrder = new Map([
			[alpha.directory, 0],
			[beta.directory, 1],
		])

		expect(
			buildSidebarItems({
				projects: [beta, alpha],
				agents: [alphaOlder, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
				projectOrder: stableOrder,
			}),
		).toEqual([
			{ type: "project", project: alpha, sessions: [alphaOlder] },
			{ type: "project", project: beta, sessions: [betaOnly] },
		])
	})

	test("recent-projects mode returns project-only rows ordered by project activity", () => {
		const preferences: SidebarPreferences = { organization: "recent-projects", sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
			}),
		).toEqual([
			{ type: "project", project: beta, sessions: [] },
			{ type: "project", project: alpha, sessions: [] },
		])
	})

	test("chronological mode sorts sessions by last activity across projects", () => {
		const preferences: SidebarPreferences = { organization: "chronological", sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map(),
				preferences,
			}),
		).toEqual([
			{ type: "session", agent: alphaNewer, project: alpha },
			{ type: "session", agent: betaOnly, project: beta },
			{ type: "session", agent: alphaOlder, project: alpha },
		])
	})

	test("created sort orders sessions by creation time instead of update time", () => {
		const preferences: SidebarPreferences = { organization: "chronological", sort: "created" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map(),
				preferences,
			}),
		).toEqual([
			{ type: "session", agent: betaOnly, project: beta },
			{ type: "session", agent: alphaNewer, project: alpha },
			{ type: "session", agent: alphaOlder, project: alpha },
		])
	})

	test("hidden project directories remove project sections and their sessions", () => {
		const preferences: SidebarPreferences = { organization: "by-project", sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
				hiddenProjectDirectories: new Set([alpha.directory]),
			}),
		).toEqual([{ type: "project", project: beta, sessions: [betaOnly] }])
	})

	test("hidden project directories remove sessions from chronological mode", () => {
		const preferences: SidebarPreferences = { organization: "chronological", sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map(),
				preferences,
				hiddenProjectDirectories: new Set([alpha.directory]),
			}),
		).toEqual([{ type: "session", agent: betaOnly, project: beta }])
	})
})
