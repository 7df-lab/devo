/**
 * Settings tab for MCP server connections.
 * Lists runtime servers from `mcp/list`, shows tools, and toggles enablement.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Switch } from "@devo/ui/components/switch"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue, useStore } from "jotai"
import { PlugIcon, PlusIcon, RefreshCwIcon } from "lucide-react"
import { useRef, useState } from "react"
import { toast } from "sonner"
import { activeServerIdAtom, serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"
import { openMcpConfigFile } from "../../services/backend"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"
import { McpReconnectButton } from "./mcp-reconnect-button"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

interface McpServerRow {
	name: string
	status: string
	toolCount: number
}

interface McpToolRow {
	name: string
	description: string
}

function mcpServerEnabled(status: string): boolean {
	return status !== "disabled"
}

interface McpSettingsProps {
	embedded?: boolean
	searchQuery?: string
}

export function McpSettings(props: McpSettingsProps) {
	const discovery = useAtomValue(discoveryAtom)
	const directory = discovery.projects[0]?.worktree ?? null
	const serverId = useAtomValue(activeServerIdAtom)
	return (
		<McpSettingsContent
			key={JSON.stringify([serverId, directory])}
			{...props}
			directory={directory}
			serverId={serverId}
		/>
	)
}

function McpSettingsContent({
	embedded = false,
	searchQuery = "",
	directory,
	serverId,
}: McpSettingsProps & { directory: string | null; serverId: string }) {
	const connected = useAtomValue(serverConnectedAtom)
	const store = useStore()
	const client = connected
		? ((directory ? getProjectClient(directory) : null) ?? getBaseClient())
		: null
	const submitting = useRef(false)
	const queryClient = useQueryClient()
	const [expanded, setExpanded] = useState<string | null>(null)
	const [openingConfig, setOpeningConfig] = useState(false)

	const { data: servers = [], isLoading, error, refetch } = useQuery({
		queryKey: ["mcp-servers", directory, serverId],
		queryFn: async (): Promise<McpServerRow[]> => {
			if (!client) return []
			const result = await client.mcp.list()
			return ((result.data ?? []) as Array<Record<string, unknown>>).map((server) => ({
				name: String(server.name ?? ""),
				status: String(server.status ?? "unknown"),
				toolCount: Number(server.toolCount ?? server.tool_count ?? 0),
			}))
		},
		enabled: connected && !!client,
	})

	const {
		data: tools = [],
		isLoading: toolsLoading,
		error: toolsError,
	} = useQuery({
		queryKey: ["mcp-tools", directory, expanded, serverId],
		queryFn: async (): Promise<McpToolRow[]> => {
			if (!client || !expanded) return []
			const result = await client.mcp.tools({ name: expanded })
			const rows = Array.isArray(result.data) ? result.data : []
			const mapped = (rows as Array<Record<string, unknown>>).map((tool) => ({
				name: String(tool.name ?? ""),
				description: String(tool.description ?? ""),
			}))
			void queryClient.invalidateQueries({ queryKey: ["mcp-servers", directory, serverId] })
			return mapped
		},
		enabled: connected && !!client && !!expanded,
	})

	const toggle = useMutation({
		mutationFn: async (params: { name: string; enabled: boolean }) => {
			if (!store.get(serverConnectedAtom)) throw new Error("Reconnect to change MCP servers.")
			if (
				store.get(activeServerIdAtom) !== serverId ||
				(store.get(discoveryAtom).projects[0]?.worktree ?? null) !== directory
			) {
				throw new Error("The active workspace changed. Try again from the current workspace.")
			}
			const currentClient = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
			if (!currentClient) throw new Error("Reconnect to change MCP servers.")
			await currentClient.mcp.setEnabled(params)
		},
		retry: false,
		onSuccess: (_, params) => {
			if (!params.enabled) {
				setExpanded((current) => (current === params.name ? null : current))
			}
		},
		onSettled: async (_, __, params) => {
			await Promise.all([
				queryClient.invalidateQueries({ queryKey: ["mcp-servers", directory, serverId] }),
				queryClient.invalidateQueries({ queryKey: ["mcp-tools", directory, params.name, serverId] }),
			])
		},
	})

	function submit(params: { name: string; enabled: boolean }) {
		if (submitting.current || !store.get(serverConnectedAtom)) return
		submitting.current = true
		void toggle
			.mutateAsync(params)
			.catch(() => {
				// Keep the original intent available in the inline retry message.
			})
			.finally(() => {
				submitting.current = false
			})
	}

	const openMcpConfig = async () => {
		if (!isElectron) {
			toast.error("MCP config can only be edited in the desktop app")
			return
		}
		setOpeningConfig(true)
		try {
			await openMcpConfigFile()
		} catch (err) {
			toast.error("Failed to open MCP config", {
				description: err instanceof Error ? err.message : String(err),
			})
		} finally {
			setOpeningConfig(false)
		}
	}

	const visibleServers = servers.filter((server) => {
		if (!searchQuery) return true
		const haystack = `${server.name} ${server.status}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})

	const addButton = (
		<Button
			type="button"
			size="sm"
			variant="secondary"
			className="h-7"
			disabled={openingConfig}
			onClick={() => void openMcpConfig()}
			aria-label="Add MCP"
		>
			<PlusIcon className="size-3.5 stroke-[1.5]" />
			Add
		</Button>
	)

	return (
		<div className={embedded ? "space-y-6" : "space-y-8"}>
			{!embedded && (
				<SettingsHeader
					title="MCP"
					action={addButton}
					description={
						<>
							Connect Model Context Protocol servers. Add servers in{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">config.toml</code>, then
							enable them here.
						</>
					}
				/>
			)}

			<SettingsSection title="Servers" action={embedded ? addButton : undefined}>
				{!connected && (
					<SettingsRow label="Runtime disconnected" description="Reconnect to change MCP servers.">
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
									: "Could not update the MCP server. Try again."
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
					<SettingsRow label="Loading" description="Fetching MCP server status">
						<RefreshCwIcon className="size-4 animate-spin text-muted-foreground" />
					</SettingsRow>
				)}
				{error && (
					<SettingsRow label="Failed to load servers" description={String(error)}>
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
				{connected && !isLoading && !error && visibleServers.length === 0 && (
					<SettingsRow
						label={servers.length === 0 ? "No MCP servers" : "No matching servers"}
						description={
							servers.length === 0
								? "Add a server with +, then restart the session."
								: "Try a different search."
						}
					>
						<PlugIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleServers.map((server) => {
					const enabled = mcpServerEnabled(server.status)
					const expandedThis = expanded === server.name
					return (
						<div key={`${directory ?? ""}:${server.name}`}>
							<SettingsRow
								label={server.name}
								description={
									enabled ? `${server.status} · ${server.toolCount} tool${server.toolCount === 1 ? "" : "s"}` : server.status
								}
							>
								<div className="flex items-center gap-2">
									<Badge variant="secondary">{server.status}</Badge>
									{enabled && (
										<McpReconnectButton
											client={client}
											connected={connected}
											directory={directory}
											server={server}
										/>
									)}
									{enabled && (
										<Button
											size="sm"
											variant="ghost"
											onClick={() =>
												setExpanded((current) => (current === server.name ? null : server.name))
											}
										>
											Tools
										</Button>
									)}
									<Switch
										checked={enabled}
										aria-label={`${enabled ? "Disable" : "Enable"} ${server.name}`}
										disabled={!connected || !client || toggle.isPending}
										aria-busy={toggle.isPending && toggle.variables?.name === server.name}
										onCheckedChange={(checked) =>
											submit({ name: server.name, enabled: checked === true })
										}
									/>
								</div>
							</SettingsRow>
							{enabled && expandedThis && (
								<div className="space-y-1 px-4 pb-3 text-xs leading-5 text-muted-foreground">
									{toolsError ? (
										<p>Failed to load tools: {String(toolsError)}</p>
									) : toolsLoading ? (
										<p className="flex items-center gap-2">
											<RefreshCwIcon className="size-3.5 animate-spin" />
											Loading tools…
										</p>
									) : tools.length === 0 ? (
										<p>
											{server.status === "failed"
												? "Server failed to start, so no tools are available."
												: "No tools advertised."}
										</p>
									) : (
										tools.map((tool) => (
											<div key={tool.name}>
												<span className="font-medium text-foreground">{tool.name}</span>
												{tool.description ? ` — ${tool.description}` : ""}
											</div>
										))
									)}
								</div>
							)}
						</div>
					)
				})}
			</SettingsSection>
		</div>
	)
}
