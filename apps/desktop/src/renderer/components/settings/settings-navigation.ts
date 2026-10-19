import {
	BellIcon,
	BookOpenIcon,
	GitForkIcon,
	InfoIcon,
	PlugIcon,
	ServerIcon,
	SettingsIcon,
	WrenchIcon,
} from "lucide-react"

type SettingsTab =
	| "general"
	| "servers"
	| "providers"
	| "models"
	| "mcp"
	| "skills"
	| "notifications"
	| "worktrees"
	| "setup"
	| "about"

type SettingsTabDefinition = { id: SettingsTab; label: string; icon: typeof SettingsIcon }
type SettingsTabGroup = { label: string; tabs: SettingsTabDefinition[] }

export const settingsGroups: SettingsTabGroup[] = [
	{
		label: "App",
		tabs: [
			{ id: "general", label: "General", icon: SettingsIcon },
			{ id: "notifications", label: "Notifications", icon: BellIcon },
			{ id: "about", label: "About", icon: InfoIcon },
		],
	},
	{
		label: "Connections",
		tabs: [
			{ id: "servers", label: "Servers", icon: ServerIcon },
			{ id: "providers", label: "Models & providers", icon: PlugIcon },
			{ id: "mcp", label: "MCP", icon: PlugIcon },
		],
	},
	{
		label: "Workspace",
		tabs: [
			{ id: "skills", label: "Skills", icon: BookOpenIcon },
			{ id: "worktrees", label: "Worktrees", icon: GitForkIcon },
			{ id: "setup", label: "Setup", icon: WrenchIcon },
		],
	},
]
