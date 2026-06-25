import type { Agent, SidebarProject } from "../../lib/types"

export type SidebarOrganization = "by-project" | "recent-projects" | "chronological"
export type SidebarSort = "updated" | "created"

export interface SidebarPreferences {
	organization: SidebarOrganization
	sort: SidebarSort
}

export const DEFAULT_SIDEBAR_PREFERENCES: SidebarPreferences = {
	organization: "by-project",
	sort: "updated",
}

export type SidebarDisplayItem =
	| {
			type: "project"
			project: SidebarProject
			sessions: Agent[]
	  }
	| {
			type: "session"
			agent: Agent
			project: SidebarProject | null
	  }

export interface BuildSidebarItemsArgs {
	projects: SidebarProject[]
	agents: Agent[]
	projectSessionsByDirectory: Map<string, Agent[]>
	preferences: SidebarPreferences
	hiddenProjectDirectories?: ReadonlySet<string>
	projectOrder?: ReadonlyMap<string, number>
}

function sortSessions(sessions: Agent[], sort: SidebarSort): Agent[] {
	const sorted = [...sessions]
	sorted.sort((a, b) => {
		const aTime = sort === "created" ? a.createdAt : a.lastActiveAt
		const bTime = sort === "created" ? b.createdAt : b.lastActiveAt
		const timeDiff = bTime - aTime
		if (timeDiff !== 0) return timeDiff
		return a.name.localeCompare(b.name)
	})
	return sorted
}

function sortRecentProjects(projects: SidebarProject[]): SidebarProject[] {
	const sorted = [...projects]
	sorted.sort((a, b) => {
		const timeDiff = b.lastActiveAt - a.lastActiveAt
		if (timeDiff !== 0) return timeDiff
		return a.name.localeCompare(b.name)
	})
	return sorted
}

function sortProjectsByStableOrder(
	projects: SidebarProject[],
	projectOrder: ReadonlyMap<string, number> | undefined,
): SidebarProject[] {
	if (!projectOrder) return projects

	const sorted = [...projects]
	sorted.sort((a, b) => {
		const orderA = projectOrder.get(a.directory) ?? Number.MAX_SAFE_INTEGER
		const orderB = projectOrder.get(b.directory) ?? Number.MAX_SAFE_INTEGER
		const orderDiff = orderA - orderB
		if (orderDiff !== 0) return orderDiff

		const nameDiff = a.name.localeCompare(b.name)
		if (nameDiff !== 0) return nameDiff
		return a.directory.localeCompare(b.directory)
	})
	return sorted
}

function buildProjectLookup(projects: SidebarProject[]): Map<string, SidebarProject> {
	const lookup = new Map<string, SidebarProject>()
	for (const project of projects) {
		lookup.set(project.directory, project)
	}
	return lookup
}

export function buildSidebarItems({
	projects,
	agents,
	projectSessionsByDirectory,
	preferences,
	hiddenProjectDirectories,
	projectOrder,
}: BuildSidebarItemsArgs): SidebarDisplayItem[] {
	const isProjectHidden = (directory: string) => hiddenProjectDirectories?.has(directory) ?? false
	const isAgentHidden = (agent: Agent) =>
		isProjectHidden(agent.projectDirectory) || isProjectHidden(agent.directory)
	const visibleProjects = projects.filter((project) => !isProjectHidden(project.directory))
	const visibleAgents = agents.filter((agent) => !isAgentHidden(agent))

	if (preferences.organization === "chronological") {
		const projectLookup = buildProjectLookup(visibleProjects)
		return sortSessions(visibleAgents, preferences.sort).map((agent) => ({
			type: "session",
			agent,
			project: projectLookup.get(agent.projectDirectory) ?? projectLookup.get(agent.directory) ?? null,
		}))
	}

	const projectRows =
		preferences.organization === "recent-projects"
			? sortRecentProjects(visibleProjects)
			: sortProjectsByStableOrder(visibleProjects, projectOrder)

	return projectRows.map((project) => ({
		type: "project",
		project,
		sessions:
			preferences.organization === "recent-projects"
				? []
				: sortSessions(
						(projectSessionsByDirectory.get(project.directory) ?? []).filter(
							(agent) => !isAgentHidden(agent),
						),
						preferences.sort,
					),
	}))
}
