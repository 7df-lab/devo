import { SidebarContent, SidebarFooter } from "@devo/ui/components/sidebar"
import { cn } from "@devo/ui/lib/utils"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue } from "jotai"
import {
	ChevronDownIcon,
	Clock3Icon,
	Loader2Icon,
	PenLineIcon,
	SearchIcon,
	SettingsIcon,
} from "lucide-react"
import { memo, useCallback, useMemo, useRef, useState, type ReactNode } from "react"
import { activeServerConfigAtom } from "../../atoms/connection"
import { sandboxMappingsAtom } from "../../atoms/derived/agents"
import { automationsEnabledAtom } from "../../atoms/feature-flags"
import { projectPaginationFamily } from "../../atoms/sessions"
import type { Agent, SidebarProject } from "../../lib/types"
import { openInTarget } from "../../services/backend"
import { loadMoreProjectSessions, loadProjectSessions } from "../../services/connection-manager"
import {
	buildSidebarItems,
	type SidebarDisplayItem,
	type SidebarPreferences,
} from "./sidebar-data"
import { AddProjectMenu, SidebarMainMenu } from "./sidebar-menus"
import {
	hiddenSidebarProjectDirectoriesAtom,
	sidebarPreferencesAtom,
} from "./sidebar-preferences"
import { ProjectRow, SessionRow } from "./sidebar-rows"

interface AppSidebarContentProps {
	agents: Agent[]
	projects: SidebarProject[]
	onOpenCommandPalette: () => void
	onAddProject?: () => void
	onRenameSession?: (agent: Agent, title: string) => Promise<void>
	onDeleteSession?: (agent: Agent) => Promise<void>
	onForkSession?: (agent: Agent) => Promise<void>
	serverConnected: boolean
}

function groupAgentsByProject(agents: Agent[]): Map<string, Agent[]> {
	const grouped = new Map<string, Agent[]>()
	for (const agent of agents) {
		if (agent.parentId) continue
		const directory = agent.projectDirectory || agent.directory
		const existing = grouped.get(directory)
		if (existing) {
			existing.push(agent)
		} else {
			grouped.set(directory, [agent])
		}
	}
	return grouped
}

const sidebarPrimaryIconClass = "size-4 stroke-[1.6]"

function TopActionRow({
	children,
	icon,
	onClick,
}: {
	children: ReactNode
	icon: ReactNode
	onClick: () => void
}) {
	return (
		<button
			type="button"
			onClick={onClick}
			className="flex h-8 w-full items-center gap-2.5 rounded-lg px-1.5 text-left text-sm font-normal text-sidebar-foreground transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06]"
		>
			<span className="flex size-4 shrink-0 items-center justify-center text-sidebar-foreground/90">
				{icon}
			</span>
			<span className="min-w-0 flex-1 truncate">{children}</span>
		</button>
	)
}

function ProjectSection({
	item,
	selectedProjectSlug,
	selectedSessionId,
	preferences,
	isCollapsed,
	onToggleCollapsed,
	onRevealInFinder,
	onRemoveProject,
	sandboxDirs,
	onRenameSession,
	onDeleteSession,
	onForkSession,
}: {
	item: Extract<SidebarDisplayItem, { type: "project" }>
	selectedProjectSlug: string | undefined
	selectedSessionId: string | null
	preferences: SidebarPreferences
	isCollapsed: boolean
	onToggleCollapsed: (directory: string) => void
	onRevealInFinder?: (directory: string) => void
	onRemoveProject: (project: SidebarProject) => void
	sandboxDirs: Set<string> | undefined
	onRenameSession?: (agent: Agent, title: string) => Promise<void>
	onDeleteSession?: (agent: Agent) => Promise<void>
	onForkSession?: (agent: Agent) => Promise<void>
}) {
	const navigate = useNavigate()
	const pagination = useAtomValue(projectPaginationFamily(item.project.directory))
	const isRecentProjects = preferences.organization === "recent-projects"
	const canShowSessions = !isRecentProjects

	const handleProjectSelect = useCallback(() => {
		if (canShowSessions && !isCollapsed && !pagination.loaded && !pagination.loading) {
			loadProjectSessions(item.project.directory, sandboxDirs, { limit: 5, roots: true })
		}
		navigate({
			to: "/project/$projectSlug",
			params: { projectSlug: item.project.slug },
		})
	}, [
		item.project.directory,
		item.project.slug,
		navigate,
		pagination.loaded,
		pagination.loading,
		sandboxDirs,
		canShowSessions,
		isCollapsed,
	])

	const handleNewChat = useCallback(() => {
		navigate({
			to: "/project/$projectSlug",
			params: { projectSlug: item.project.slug },
		})
	}, [item.project.slug, navigate])

	const handleToggleCollapsed = useCallback(() => {
		if (canShowSessions && isCollapsed && !pagination.loaded && !pagination.loading) {
			loadProjectSessions(item.project.directory, sandboxDirs, { limit: 5, roots: true })
		}
		onToggleCollapsed(item.project.directory)
	}, [
		canShowSessions,
		isCollapsed,
		item.project.directory,
		onToggleCollapsed,
		pagination.loaded,
		pagination.loading,
		sandboxDirs,
	])

	const handleLoadMore = useCallback(() => {
		loadMoreProjectSessions(item.project.directory, pagination.currentLimit)
	}, [item.project.directory, pagination.currentLimit])

	const handleRevealInFinder = useCallback(() => {
		onRevealInFinder?.(item.project.directory)
	}, [item.project.directory, onRevealInFinder])

	return (
		<section className="flex flex-col">
			<ProjectRow
				project={item.project}
				isSelected={selectedProjectSlug === item.project.slug && !selectedSessionId}
				showCount={isRecentProjects}
				isCollapsed={isCollapsed}
				canToggleSessions={canShowSessions}
				onSelect={handleProjectSelect}
				onToggleCollapsed={handleToggleCollapsed}
				onNewChat={handleNewChat}
				onRevealInFinder={handleRevealInFinder}
				onRemoveProject={() => onRemoveProject(item.project)}
			/>
			{canShowSessions && (
				<div
					aria-hidden={isCollapsed}
					className={cn(
						"grid transition-[grid-template-rows,opacity] duration-200 ease-out motion-reduce:transition-none",
						isCollapsed ? "grid-rows-[0fr] opacity-0" : "grid-rows-[1fr] opacity-100",
					)}
					inert={isCollapsed}
				>
					<div className={cn("min-h-0 overflow-hidden", isCollapsed && "pointer-events-none")}>
						<div className="flex flex-col gap-y-1">
							{pagination.loading && item.sessions.length === 0 && (
								<div className="flex h-8 items-center gap-2 py-1 pr-1.5 pl-[34px] text-xs text-muted-foreground">
									<Loader2Icon className="size-3.5 animate-spin" />
									Loading
								</div>
							)}
							{item.sessions.map((agent) => (
								<SessionRow
									key={agent.id}
									agent={agent}
									isSelected={agent.id === selectedSessionId}
									onRename={onRenameSession}
									onDelete={onDeleteSession}
									onFork={onForkSession}
								/>
							))}
							{pagination.loaded && pagination.hasMore && item.sessions.length > 0 && (
								<button
									type="button"
									onClick={handleLoadMore}
									disabled={pagination.loading}
									className="h-8 rounded-lg py-1 pr-1.5 pl-7 text-left text-[13px] font-normal text-muted-foreground transition-colors hover:bg-black/[0.03] hover:text-muted-foreground/90 disabled:opacity-60 dark:hover:bg-white/[0.05]"
								>
									{pagination.loading ? "Loading..." : "Show more"}
								</button>
							)}
						</div>
					</div>
				</div>
			)}
		</section>
	)
}

const ChronologicalSessionList = memo(function ChronologicalSessionList({
	items,
	selectedSessionId,
	onRenameSession,
	onDeleteSession,
	onForkSession,
}: {
	items: Extract<SidebarDisplayItem, { type: "session" }>[]
	selectedSessionId: string | null
	onRenameSession?: (agent: Agent, title: string) => Promise<void>
	onDeleteSession?: (agent: Agent) => Promise<void>
	onForkSession?: (agent: Agent) => Promise<void>
}) {
	return (
		<div className="flex flex-col gap-y-1">
			{items.map((item) => (
				<SessionRow
					key={item.agent.id}
					agent={item.agent}
					isSelected={item.agent.id === selectedSessionId}
					onRename={onRenameSession}
					onDelete={onDeleteSession}
					onFork={onForkSession}
					showProject
				/>
			))}
		</div>
	)
})

export function AppSidebarContent({
	agents,
	projects,
	onOpenCommandPalette,
	onAddProject,
	onRenameSession,
	onDeleteSession,
	onForkSession,
	serverConnected,
}: AppSidebarContentProps) {
	const navigate = useNavigate()
	const routeParams = useParams({ strict: false }) as { projectSlug?: string; sessionId?: string }
	const selectedSessionId = routeParams.sessionId ?? null
	const [preferences, setPreferences] = useAtom(sidebarPreferencesAtom)
	const [hiddenProjectDirectories, setHiddenProjectDirectories] = useAtom(
		hiddenSidebarProjectDirectoriesAtom,
	)
	const [collapsedProjectDirs, setCollapsedProjectDirs] = useState<Set<string>>(() => new Set())
	const { parentToSandboxes } = useAtomValue(sandboxMappingsAtom)
	const automationsEnabled = useAtomValue(automationsEnabledAtom)
	const activeServer = useAtomValue(activeServerConfigAtom)
	const isLocalServer = activeServer.type === "local"
	const canRevealInFinder = typeof window !== "undefined" && "devo" in window
	const stableProjectOrderRef = useRef<Map<string, number>>(new Map())

	const visibleAgents = useMemo(() => agents.filter((agent) => !agent.parentId), [agents])
	const hiddenProjectDirectorySet = useMemo(
		() => new Set(hiddenProjectDirectories),
		[hiddenProjectDirectories],
	)
	const stableProjectOrder = useMemo(() => {
		const order = stableProjectOrderRef.current
		for (const project of projects) {
			if (!order.has(project.directory)) {
				order.set(project.directory, order.size)
			}
		}
		return order
	}, [projects])
	const projectSessionsByDirectory = useMemo(
		() => groupAgentsByProject(visibleAgents),
		[visibleAgents],
	)
	const sidebarItems = useMemo(
		() =>
			buildSidebarItems({
				projects,
				agents: visibleAgents,
				projectSessionsByDirectory,
				preferences,
				hiddenProjectDirectories: hiddenProjectDirectorySet,
				projectOrder: stableProjectOrder,
			}),
		[
			projects,
			visibleAgents,
			projectSessionsByDirectory,
			preferences,
			hiddenProjectDirectorySet,
			stableProjectOrder,
		],
	)

	const hasContent = sidebarItems.length > 0

	const handleNewChat = useCallback(() => {
		if (routeParams.projectSlug) {
			navigate({
				to: "/project/$projectSlug",
				params: { projectSlug: routeParams.projectSlug },
			})
			return
		}
		navigate({ to: "/" })
	}, [navigate, routeParams.projectSlug])

	const handleToggleProjectCollapsed = useCallback((directory: string) => {
		setCollapsedProjectDirs((previous) => {
			const next = new Set(previous)
			if (next.has(directory)) {
				next.delete(directory)
			} else {
				next.add(directory)
			}
			return next
		})
	}, [])

	const handleRevealInFinder = useCallback((directory: string) => {
		openInTarget(directory, "finder", false).catch((error) => {
			console.error("Failed to reveal project in Finder", error)
		})
	}, [])

	const handleRemoveProject = useCallback(
		(project: SidebarProject) => {
			setHiddenProjectDirectories((previous) =>
				previous.includes(project.directory) ? previous : [...previous, project.directory],
			)
			setCollapsedProjectDirs((previous) => {
				if (!previous.has(project.directory)) return previous
				const next = new Set(previous)
				next.delete(project.directory)
				return next
			})
			if (routeParams.projectSlug === project.slug) {
				navigate({ to: "/" })
			}
		},
		[navigate, routeParams.projectSlug, setHiddenProjectDirectories],
	)

	return (
		<>
			<SidebarContent className="gap-0 bg-transparent px-0 pb-3">
				<div className="flex shrink-0 flex-col gap-1 px-3 pb-7">
					<TopActionRow
						icon={<PenLineIcon className={sidebarPrimaryIconClass} />}
						onClick={handleNewChat}
					>
						New chat
					</TopActionRow>
					<TopActionRow
						icon={<SearchIcon className={sidebarPrimaryIconClass} />}
						onClick={onOpenCommandPalette}
					>
						Search
					</TopActionRow>
					{automationsEnabled && isLocalServer && (
						<TopActionRow
							icon={<Clock3Icon className={sidebarPrimaryIconClass} />}
							onClick={() => navigate({ to: "/automations" })}
						>
							Automations
						</TopActionRow>
					)}
				</div>

				<div className="group/projects-header flex h-9 shrink-0 items-center gap-1 px-4">
					<div className="flex min-w-0 flex-1 items-center gap-1 text-sm font-normal text-muted-foreground/60 transition-colors group-hover/projects-header:text-muted-foreground/75 group-focus-within/projects-header:text-muted-foreground/75">
						<span className="truncate">Projects</span>
						<ChevronDownIcon className="size-3.5 shrink-0 opacity-0 transition-opacity duration-150 group-hover/projects-header:opacity-100 group-focus-within/projects-header:opacity-100" />
					</div>
					<div className="flex items-center gap-1 opacity-0 transition-opacity duration-150 group-hover/projects-header:opacity-100 group-focus-within/projects-header:opacity-100">
						<SidebarMainMenu
							preferences={preferences}
							onPreferencesChange={setPreferences}
							onOpenCommandPalette={onOpenCommandPalette}
						/>
						<AddProjectMenu onAddExistingFolder={onAddProject} />
					</div>
				</div>

				{!hasContent && (
					<div className="flex flex-1 items-center justify-center p-6">
						<div className="flex flex-col gap-1 text-center">
							<p className="text-sm text-muted-foreground">
								{serverConnected ? "No projects yet" : "Server offline"}
							</p>
							<p className="text-xs text-muted-foreground/70">
								{serverConnected
									? "Add a project to get started"
									: "Check your connection in Settings"}
							</p>
						</div>
					</div>
				)}

				{hasContent && (
					<div className="scrollbar-comfort flex min-h-0 flex-1 flex-col gap-4 overflow-auto px-3 pb-2">
						{preferences.organization === "chronological" ? (
							<ChronologicalSessionList
								items={sidebarItems.filter(
									(item): item is Extract<SidebarDisplayItem, { type: "session" }> =>
										item.type === "session",
								)}
								selectedSessionId={selectedSessionId}
								onRenameSession={onRenameSession}
								onDeleteSession={onDeleteSession}
								onForkSession={onForkSession}
							/>
						) : (
							<div className="flex flex-col gap-1">
								{sidebarItems.map((item) => {
									if (item.type !== "project") return null
									return (
										<ProjectSection
											key={item.project.id}
											item={item}
											selectedProjectSlug={routeParams.projectSlug}
											selectedSessionId={selectedSessionId}
											preferences={preferences}
											isCollapsed={collapsedProjectDirs.has(item.project.directory)}
											onToggleCollapsed={handleToggleProjectCollapsed}
											onRemoveProject={handleRemoveProject}
											onRevealInFinder={canRevealInFinder ? handleRevealInFinder : undefined}
											sandboxDirs={parentToSandboxes.get(item.project.directory)}
											onRenameSession={onRenameSession}
											onDeleteSession={onDeleteSession}
											onForkSession={onForkSession}
										/>
									)
								})}
							</div>
						)}
					</div>
				)}
			</SidebarContent>

			<SidebarFooter className="gap-1 px-3 pt-0 pb-3">
				<button
					type="button"
					onClick={() => navigate({ to: "/settings" })}
					className={cn(
						"flex h-8 w-full items-center gap-2.5 rounded-lg px-1.5 text-left text-sm font-normal text-muted-foreground transition-colors hover:bg-black/[0.04] hover:text-sidebar-foreground dark:hover:bg-white/[0.06]",
					)}
				>
					<SettingsIcon className={sidebarPrimaryIconClass} />
					<span className="truncate">Settings</span>
				</button>
			</SidebarFooter>
		</>
	)
}
