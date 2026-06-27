import type { SidebarProject } from "./types"

/**
 * Resolves the project directory NewChat should target.
 *
 * Route context wins first. Without route context, keep the user's current
 * explicit selection if it still exists, then avoid falling back to the
 * filesystem root when a real project is available.
 */
export function resolveSelectedProjectDirectory(
	projects: SidebarProject[],
	projectSlug: string | undefined,
	currentDirectory: string,
	options: {
		preserveCurrentDirectory?: boolean
		unavailableDirectories?: ReadonlySet<string>
	} = {},
): string {
	if (projects.length === 0) return ""

	if (projectSlug) {
		const routeProject = projects.find((project) => project.slug === projectSlug)
		if (routeProject) return routeProject.directory
	}

	if (
		options.preserveCurrentDirectory &&
		currentDirectory &&
		!options.unavailableDirectories?.has(currentDirectory) &&
		projects.some((project) => project.directory === currentDirectory)
	) {
		return currentDirectory
	}

	return ""
}
