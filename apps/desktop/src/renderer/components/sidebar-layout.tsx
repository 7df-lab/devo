/**
 * Sidebar shell layout: wraps child routes with the sidebar + SidebarInset chrome.
 * Reads from SidebarSlotContext to allow child routes to override sidebar content.
 */
import { Button } from "@devo/ui/components/button"
import {
	Sidebar,
	SidebarHeader,
	SidebarInset,
	SidebarProvider,
	useSidebar,
} from "@devo/ui/components/sidebar"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { Outlet, useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import { PanelLeftIcon } from "lucide-react"
import { type MouseEvent, useCallback, useEffect, useRef } from "react"
import { serverConnectedAtom } from "../atoms/connection"
import { terminalPanelOpenAtom } from "../atoms/terminal"
import { useAgents, useProjectList, useSetCommandPaletteOpen } from "../hooks/use-agents"
import { useAgentActions } from "../hooks/use-server"
import { formatShortcut } from "../lib/shortcut-display"
import type { Agent } from "../lib/types"
import { pickDirectory } from "../services/backend"
import { loadProjectSessions } from "../services/connection-manager"
import { APP_BAR_HEIGHT, AppBar } from "./app-bar"
import { DesktopTerminalPanel } from "./desktop-terminal-panel"
import { AppSidebarContent } from "./sidebar"
import { hiddenSidebarProjectDirectoriesAtom } from "./sidebar/sidebar-preferences"
import { useSidebarSlot } from "./sidebar-slot-context"
import { UpdateBanner } from "./update-banner"

// ============================================================
// Constants
// ============================================================

const isMac =
	typeof window !== "undefined" && "devo" in window && window.devo.platform === "darwin"
const isElectronEnv = typeof window !== "undefined" && "devo" in window
const isWindowsElectron = isElectronEnv && window.devo.platform === "win32"

/** Pixel offset from the left edge where window controls start */
const WINDOW_CONTROLS_LEFT = isMac && isElectronEnv ? 93 : 8
/** Total width reserved for traffic lights or custom titlebar controls */
const WINDOW_CONTROLS_INSET = isMac && isElectronEnv ? 160 : isWindowsElectron ? 200 : 72
const WINDOW_CONTROLS_RIGHT_INSET = isWindowsElectron ? 138 : 12

// ============================================================
// NarrowWindowCollapser
// ============================================================

/**
 * Watches the window width and auto-collapses the sidebar when it drops below
 * COLLAPSE_THRESHOLD px, restoring it when the window grows back above the threshold.
 * Must be rendered inside a <SidebarProvider>.
 */
const COLLAPSE_THRESHOLD = 600

function NarrowWindowCollapser() {
	const { open, setOpen } = useSidebar()
	// Track whether the last collapse was triggered by us (vs. the user manually toggling)
	const collapsedByUsRef = useRef(false)

	useEffect(() => {
		const check = () => {
			const narrow = window.innerWidth < COLLAPSE_THRESHOLD
			if (narrow && open) {
				collapsedByUsRef.current = true
				setOpen(false)
			} else if (!narrow && !open && collapsedByUsRef.current) {
				// Only re-open if WE collapsed it — don't override the user's manual close
				collapsedByUsRef.current = false
				setOpen(true)
			} else if (!narrow) {
				// Window grew back — reset the flag regardless so we don't re-open unexpectedly
				if (!open) collapsedByUsRef.current = false
			}
		}

		check()
		window.addEventListener("resize", check)
		return () => window.removeEventListener("resize", check)
	}, [open, setOpen])

	return null
}

// ============================================================
// WindowControls
// ============================================================

const APP_MENU_ITEMS = [
	{ id: "edit", label: "Edit" },
	{ id: "view", label: "View" },
	{ id: "window", label: "Window" },
] as const

function AppMenuBar() {
	const handleMenuClick = useCallback(
		(event: MouseEvent<HTMLButtonElement>, id: (typeof APP_MENU_ITEMS)[number]["id"]) => {
			const rect = event.currentTarget.getBoundingClientRect()
			void window.devo.appMenu.popup(id, {
				x: Math.round(rect.left),
				y: Math.round(rect.bottom),
			})
		},
		[],
	)

	if (!isWindowsElectron) {
		return null
	}

	return (
		<nav aria-label="Application menu" className="ml-2 flex items-center gap-0.5">
			{APP_MENU_ITEMS.map((item) => (
				<button
					key={item.id}
					type="button"
					className="h-7 rounded-md px-2 text-sm font-normal text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
					onClick={(event) => handleMenuClick(event, item.id)}
				>
					{item.label}
				</button>
			))}
		</nav>
	)
}

/**
 * Absolutely positioned window controls that
 * stay next to the macOS traffic lights regardless of sidebar state.
 * Must be rendered inside a SidebarProvider.
 */
function WindowControls() {
	const { toggleSidebar } = useSidebar()
	const toggleSidebarShortcut = formatShortcut(["mod", "B"])

	return (
		<div
			className="absolute z-50 flex items-center gap-0.5"
			style={{
				top: 6,
				left: WINDOW_CONTROLS_LEFT,
				// @ts-expect-error -- vendor-prefixed CSS property
				WebkitAppRegion: "no-drag",
			}}
		>
			<Tooltip>
				<TooltipTrigger
					render={
						<Button
							variant="ghost"
							size="icon"
							className="size-7 shrink-0"
							onClick={toggleSidebar}
						/>
					}
				>
					<PanelLeftIcon className="size-3.5" />
				</TooltipTrigger>
				<TooltipContent>Toggle sidebar ({toggleSidebarShortcut})</TooltipContent>
			</Tooltip>
			<AppMenuBar />
		</div>
	)
}

// ============================================================
// SidebarLayout
// ============================================================

export function SidebarLayout() {
	const navigate = useNavigate()
	const { sessionId } = useParams({ strict: false }) as { sessionId?: string }
	const { content: slotContent, footer: slotFooter } = useSidebarSlot()
	const [terminalPanelOpen, setTerminalPanelOpen] = useAtom(terminalPanelOpenAtom)

	// ---- Sidebar-specific data ----
	const agents = useAgents()
	const projects = useProjectList()
	const setCommandPaletteOpen = useSetCommandPaletteOpen()
	const setHiddenProjectDirectories = useSetAtom(hiddenSidebarProjectDirectoriesAtom)
	const { renameSession, deleteSession, forkSession } = useAgentActions()
	const serverConnected = useAtomValue(serverConnectedAtom)

	// Sub-agents are filtered at the API level (roots: true)
	const visibleAgents = agents
	const activeAgent = sessionId ? visibleAgents.find((agent) => agent.id === sessionId) : null
	const terminalDirectory = activeAgent?.worktreePath ?? activeAgent?.directory ?? null

	const handleRenameSession = useCallback(
		async (agent: Agent, title: string) => {
			await renameSession(agent.directory, agent.sessionId, title)
		},
		[renameSession],
	)

	const handleDeleteSession = useCallback(
		async (agent: Agent) => {
			await deleteSession(agent.directory, agent.sessionId)
		},
		[deleteSession],
	)

	const handleForkSession = useCallback(
		async (agent: Agent) => {
			const forked = await forkSession(agent.directory, agent.sessionId)
			if (forked) {
				navigate({
					to: "/project/$projectSlug/session/$sessionId",
					params: { projectSlug: agent.projectSlug, sessionId: forked.id },
				})
			}
		},
		[forkSession, navigate],
	)

	const handleOpenCommandPalette = useCallback(() => {
		setCommandPaletteOpen(true)
	}, [setCommandPaletteOpen])

	const handleAddProject = useCallback(async () => {
		const directory = await pickDirectory()
		if (!directory) return
		setHiddenProjectDirectories((previous) =>
			previous.includes(directory) ? previous.filter((item) => item !== directory) : previous,
		)
		await loadProjectSessions(directory)
		navigate({ to: "/" })
	}, [navigate, setHiddenProjectDirectories])

	return (
		<div
			className="relative flex h-screen text-foreground"
			style={
				{
					"--window-controls-inset": `${WINDOW_CONTROLS_INSET}px`,
					"--window-controls-right-inset": `${WINDOW_CONTROLS_RIGHT_INSET}px`,
				} as React.CSSProperties
			}
		>
			<SidebarProvider embedded defaultOpen={true}>
				<NarrowWindowCollapser />
				<Sidebar collapsible="offcanvas" variant="sidebar">
					{/* Sidebar header -- reserves space to match the app bar height so
					 * sidebar content aligns with the main content area. Also clears
					 * the traffic lights + the absolutely-positioned toggle button. */}
					<SidebarHeader
						className="flex-row items-center gap-1 shrink-0 transition-colors duration-150"
						style={{
							height: APP_BAR_HEIGHT,
							// Make header draggable on Electron (acts as title bar above sidebar)
							// @ts-expect-error -- vendor-prefixed CSS property
							WebkitAppRegion: "drag",
						}}
					/>
					{slotContent ?? (
					<AppSidebarContent
						agents={visibleAgents}
						projects={projects}
						onOpenCommandPalette={handleOpenCommandPalette}
						onAddProject={handleAddProject}
						onRenameSession={handleRenameSession}
						onDeleteSession={handleDeleteSession}
						onForkSession={handleForkSession}
						serverConnected={serverConnected}
					/>
					)}
					{/* Footer: false = hide, ReactNode = render it, null = let default handle it.
					 * When default sidebar is active, AppSidebarContent renders its own footer. */}
					{slotFooter !== false && slotFooter}
				</Sidebar>
				<SidebarInset>
					<UpdateBanner />
					<AppBar />
					{/* Flex-1 + min-h-0 wrapper: pages use h-full which would
					    resolve to 100% of SidebarInset, ignoring AppBar height.
					    This container takes remaining space after AppBar and
					    constrains page content correctly. */}
					<div data-slot="content-area" className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
						<Outlet />
					</div>
					<DesktopTerminalPanel
						open={terminalPanelOpen}
						directory={terminalDirectory}
						onOpenChange={setTerminalPanelOpen}
					/>
				</SidebarInset>
				{/* Rendered last so it paints on top of the sidebar and app bar,
				    whose transition properties create stacking contexts. */}
				<WindowControls />
			</SidebarProvider>
		</div>
	)
}
