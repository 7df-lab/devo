import { SidebarContent } from "@devo/ui/components/sidebar"
import { Outlet, useNavigate, useRouterState } from "@tanstack/react-router"
import { useAtomValue } from "jotai"
import { ArrowLeftIcon } from "lucide-react"
import { useEffect } from "react"
import { lastAppRouteAtom } from "../../atoms/ui"
import { resolveSettingsBackTarget } from "../../lib/app-navigation"
import { useSetSidebarSlot } from "../sidebar-slot-context"
import { TopActionRow, sidebarPrimaryIconClass } from "../sidebar/sidebar-top-action"
import { settingsGroups } from "./settings-navigation"

// ============================================================
// Settings layout (renders <Outlet /> for child routes)
// ============================================================

export function SettingsPage() {
	const { setContent, setFooter } = useSetSidebarSlot()

	useEffect(() => {
		setContent(<SettingsSidebarContent />)
		setFooter(false)
		return () => {
			setContent(null)
			setFooter(null)
		}
	}, [setContent, setFooter])

	return (
		<div className="h-full overflow-y-auto">
			<div className="mx-auto max-w-3xl px-8 py-10 sm:px-10">
				<Outlet />
			</div>
		</div>
	)
}

// ============================================================
// Sidebar content injected via slot context
// ============================================================

function SettingsSidebarContent() {
	const navigate = useNavigate()
	const pathname = useRouterState({ select: (s) => s.location.pathname })
	const lastAppRoute = useAtomValue(lastAppRouteAtom)

	// Derive active tab from the last path segment (e.g. "/settings/general" -> "general")
	const activeTab = pathname.split("/").pop() || "general"

	return (
		<SidebarContent className="gap-0 bg-transparent px-0 pb-3">
			<div className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto px-3 pb-7">
				<TopActionRow
					icon={<ArrowLeftIcon aria-hidden="true" className={sidebarPrimaryIconClass} />}
					onClick={() => navigate(resolveSettingsBackTarget(lastAppRoute))}
				>
					Back to app
				</TopActionRow>
				{settingsGroups.map((group) => (
					<div key={group.label} role="group" aria-label={group.label}>
						<div
							aria-hidden="true"
							className="px-1.5 pb-1 pt-2 text-[11px] font-medium text-sidebar-foreground/60"
						>
							{group.label}
						</div>
						<div className="flex flex-col gap-1">
							{group.tabs.map((tab) => {
								const Icon = tab.icon
								return (
									<TopActionRow
										key={tab.id}
										icon={<Icon aria-hidden="true" className={sidebarPrimaryIconClass} />}
										onClick={() => navigate({ to: `/settings/${tab.id}` })}
										isActive={activeTab === tab.id}
									>
										{tab.label}
									</TopActionRow>
								)
							})}
						</div>
					</div>
				))}
			</div>
		</SidebarContent>
	)
}
