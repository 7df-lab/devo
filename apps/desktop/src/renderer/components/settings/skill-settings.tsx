/**
 * Settings tab for discovered skills and per-skill enablement.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Switch } from "@devo/ui/components/switch"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue, useStore } from "jotai"
import { BookOpenIcon, RefreshCwIcon } from "lucide-react"
import { useRef } from "react"
import { activeServerIdAtom, serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"
import { skillSourceLabel } from "./skill-source"

interface SkillRow {
	id: string
	name: string
	description: string
	path: string
	enabled: boolean
	source: string
}

interface SkillSettingsProps {
	embedded?: boolean
	searchQuery?: string
}

export function SkillSettings(props: SkillSettingsProps) {
	const discovery = useAtomValue(discoveryAtom)
	const directory = discovery.projects[0]?.worktree ?? null
	const serverId = useAtomValue(activeServerIdAtom)
	return (
		<SkillSettingsContent
			key={JSON.stringify([serverId, directory])}
			{...props}
			directory={directory}
			serverId={serverId}
		/>
	)
}

function SkillSettingsContent({
	embedded = false,
	searchQuery = "",
	directory,
	serverId,
}: SkillSettingsProps & { directory: string | null; serverId: string }) {
	const connected = useAtomValue(serverConnectedAtom)
	const store = useStore()
	const client = connected
		? ((directory ? getProjectClient(directory) : null) ?? getBaseClient())
		: null
	const submitting = useRef(false)
	const queryClient = useQueryClient()

	const {
		data: skills = [],
		isLoading,
		error,
		refetch,
	} = useQuery({
		queryKey: ["skills-settings", directory, serverId],
		queryFn: async (): Promise<SkillRow[]> => {
			if (!client) return []
			const result = await client.app.skills()
			return ((result.data ?? []) as Array<Record<string, unknown>>).map((skill) => ({
				id: String(skill.id ?? skill.path ?? skill.name ?? ""),
				name: String(skill.name ?? ""),
				description: String(
					skill.shortDescription ?? skill.short_description ?? skill.description ?? "",
				),
				path: String(skill.path ?? ""),
				enabled: skill.enabled !== false,
				source: skillSourceLabel(skill.source),
			}))
		},
		enabled: connected && !!client,
	})

	const toggle = useMutation({
		mutationFn: async (params: { path: string; enabled: boolean; name: string }) => {
			if (!store.get(serverConnectedAtom)) throw new Error("Reconnect to change skills.")
			if (
				store.get(activeServerIdAtom) !== serverId ||
				(store.get(discoveryAtom).projects[0]?.worktree ?? null) !== directory
			) {
				throw new Error("The active workspace changed. Try again from the current workspace.")
			}
			const currentClient = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
			if (!currentClient?.app.setSkillEnabled) throw new Error("Reconnect to change skills.")
			await currentClient.app.setSkillEnabled({ path: params.path, enabled: params.enabled })
		},
		onSuccess: async () => {
			await Promise.all([
				queryClient.invalidateQueries({ queryKey: ["skills-settings"] }),
				queryClient.invalidateQueries({ queryKey: ["skills"] }),
			])
		},
	})

	function submit(params: { path: string; enabled: boolean; name: string }) {
		if (submitting.current || !store.get(serverConnectedAtom)) return
		submitting.current = true
		void toggle
			.mutateAsync(params)
			.catch(() => {
				// The mutation error is rendered below, including the original retry intent.
			})
			.finally(() => {
				submitting.current = false
			})
	}

	const visibleSkills = skills.filter((skill) => {
		if (!searchQuery) return true
		const haystack =
			`${skill.name} ${skill.description} ${skill.path} ${skill.source}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})

	return (
		<div className={embedded ? "space-y-6" : "space-y-8"}>
			{!embedded && (
				<SettingsHeader
					title="Skills"
					description={
						<>
							Skills are discovered from user, workspace, plugin, and system roots. Disable a skill
							to keep it off this machine. Insert one in chat with{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">/skills</code> or{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">@</code>.
						</>
					}
				/>
			)}

			<SettingsSection title="Installed skills">
				{!connected && (
					<SettingsRow label="Runtime disconnected" description="Reconnect to change skills.">
						<output className="text-xs text-muted-foreground">Offline</output>
					</SettingsRow>
				)}
				{toggle.isError && toggle.variables && (
					<div role="alert">
						<SettingsRow
							label={`Failed to ${toggle.variables.enabled ? "enable" : "disable"} ${toggle.variables.name}`}
							description={
								toggle.error instanceof Error
									? toggle.error.message
									: "Could not update the skill. Try again."
							}
						>
							<Button
								size="sm"
								variant="outline"
								disabled={!connected || !client}
								onClick={() => toggle.variables && submit(toggle.variables)}
							>
								Retry
							</Button>
						</SettingsRow>
					</div>
				)}
				{isLoading && (
					<SettingsRow label="Loading" description="Fetching discovered skills">
						<RefreshCwIcon className="size-4 animate-spin text-muted-foreground" />
					</SettingsRow>
				)}
				{error && (
					<SettingsRow label="Failed to load skills" description={String(error)}>
						<Button
							size="sm"
							variant="outline"
							disabled={!connected || !client}
							onClick={() => void refetch()}
						>
							Retry
						</Button>
					</SettingsRow>
				)}
				{connected && !isLoading && !error && visibleSkills.length === 0 && (
					<SettingsRow
						label={skills.length === 0 ? "No skills discovered" : "No matching skills"}
						description={
							skills.length === 0
								? "Add a SKILL.md under a skills/ directory, then reopen the session."
								: "Try a different search."
						}
					>
						<BookOpenIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleSkills.map((skill) => (
					<SettingsRow
						key={skill.path || skill.id}
						label={skill.name}
						description={skill.description || skill.path}
					>
						<div className="flex items-center gap-2">
							<Badge variant="secondary">{skill.source}</Badge>
							<Switch
								checked={skill.enabled}
								disabled={!connected || !client || !skill.path || toggle.isPending}
								aria-label={`${skill.enabled ? "Disable" : "Enable"} ${skill.name}`}
								onCheckedChange={(checked) =>
									submit({ path: skill.path, enabled: checked === true, name: skill.name })
								}
							/>
						</div>
					</SettingsRow>
				))}
			</SettingsSection>
		</div>
	)
}
