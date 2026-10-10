/** Onboarding uses the same server-owned catalog and connection flows as Settings. */
import { Button } from "@devo/ui/components/button"
import { Input } from "@devo/ui/components/input"
import { Spinner } from "@devo/ui/components/spinner"
import { CheckIcon, RefreshCwIcon } from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import {
	type CatalogProviderInfo,
	useProviderCatalog,
} from "../../../hooks/use-devo-data"
import { useModelDirectory } from "../../../hooks/use-model-directory"
import { useServerConnection } from "../../../hooks/use-server"
import { invalidateProviderDependentQueries } from "../../../lib/invalidate-provider-queries"
import { compareConnectedFirst } from "../../../lib/providers"
import { ConnectProviderDialog } from "../../settings/connect-provider-dialog"
import { ConnectionDetailDialog } from "../../settings/connection-detail-dialog"
import { isDesktopOAuthProvider } from "../../settings/desktop-oauth-providers"
import { ProviderIcon } from "../../settings/provider-icon"
import { TemplateConnectDialog } from "../../settings/template-connect-dialog"

interface ProviderSetupStepProps {
	onComplete: (count: number) => void
	onSkip: () => void
}

export function ProviderSetupStep({
	onComplete,
	onSkip,
}: ProviderSetupStepProps) {
	const { connected: serverConnected } = useServerConnection()
	const catalog = useProviderCatalog()
	const directory = useModelDirectory()
	const [search, setSearch] = useState("")
	const [connection, setConnection] = useState<CatalogProviderInfo | null>(null)
	const [detailId, setDetailId] = useState<string | null>(null)
	// Keep the OAuth provider stable while catalog refreshes complete.
	const oauthProvider = useMemo(
		() => (connection ? { ...connection, env: [] } : null),
		[connection],
	)

	useEffect(() => {
		if (serverConnected) directory.refreshCatalog("ifStale")
	}, [serverConnected, directory.refreshCatalog])

	const providers = useMemo(() => {
		const data = catalog.data
		if (!data) return []
		const terms = search.toLowerCase().trim().split(/\s+/).filter(Boolean)
		return data.providers
			.filter((provider) => {
				const text =
					`${provider.id} ${provider.name} ${provider.description ?? ""} ${Object.entries(
						provider.models ?? {},
					)
						.map(([id, model]) => `${id} ${model.name ?? ""}`)
						.join(" ")}`.toLowerCase()
				return terms.every((term) => text.includes(term))
			})
			.sort((a, b) => compareConnectedFirst(data.connectedIds, a, b))
	}, [catalog.data, search])
	const connectedCount = catalog.data?.connectedIds.size ?? 0
	const detail = catalog.data?.providers.find(
		(provider) => provider.id === detailId,
	)
	const error = catalog.error ?? directory.error
	const refresh = () => {
		catalog.reload()
		directory.refreshCatalog("force")
	}
	const changed = useCallback(() => {
		invalidateProviderDependentQueries()
		catalog.reload()
	}, [catalog.reload])
	const connected = () => {
		setConnection(null)
		changed()
	}

	if (!serverConnected) {
		return (
			<div className="flex h-full flex-col items-center justify-center gap-6 px-6 text-center">
				<Spinner className="size-6 text-muted-foreground" />
				<h2 className="text-[22px] font-medium tracking-tight">
					Connecting to Devo…
				</h2>
				<Button variant="outline" onClick={onSkip}>
					Skip for now
				</Button>
			</div>
		)
	}

	return (
		<div className="mx-auto flex min-h-full w-full max-w-2xl flex-col justify-center gap-6 px-6 py-8">
			<div className="space-y-2 text-center">
				<h2 className="text-[22px] font-medium tracking-tight">
					Connect a provider
				</h2>
				<p className="text-sm text-muted-foreground">
					Choose from the live catalog or search for a model you want to use.
				</p>
			</div>
			<div className="space-y-3">
				<div className="flex items-center gap-2">
					<Input
						aria-label="Search providers or models"
						placeholder="Search providers or models…"
						value={search}
						onChange={(event) => setSearch(event.target.value)}
						className="h-9 flex-1"
					/>
					<Button
						size="sm"
						variant="outline"
						onClick={refresh}
						disabled={directory.refreshing}
					>
						<RefreshCwIcon
							className={`size-3.5 stroke-[1.5] ${directory.refreshing ? "animate-spin" : ""}`}
							aria-hidden="true"
						/>
						{directory.refreshing ? "Refreshing…" : "Refresh catalog"}
					</Button>
				</div>
				{error && (
					<p role="alert" className="text-sm text-destructive">
						Could not refresh the catalog: {error}. You can retry or continue
						with saved providers.
					</p>
				)}
				{directory.offline && (
					<p role="status" className="text-xs text-muted-foreground">
						Offline mode · showing the saved catalog.
					</p>
				)}
				<p className="text-xs text-muted-foreground" aria-live="polite">
					{providers.length} provider{providers.length === 1 ? "" : "s"} ·{" "}
					{connectedCount} connected
				</p>
				<div
					className="grid h-[42vh] auto-rows-min grid-cols-1 gap-2 overflow-y-auto p-1 sm:grid-cols-2"
					aria-label="Provider catalog"
					aria-busy={catalog.loading}
				>
					{catalog.loading && !catalog.data ? (
						<div className="col-span-full flex justify-center py-12">
							<Spinner className="size-5 text-muted-foreground" />
						</div>
					) : (
						providers.map((provider) => {
							const isConnected =
								catalog.data?.connectedIds.has(provider.id) ?? false
							const count = Object.values(provider.models ?? {}).filter(
								(model) => model.enabled !== false,
							).length
							return (
								<button
									key={provider.id}
									type="button"
									onClick={() =>
										isConnected
											? setDetailId(provider.id)
											: setConnection(provider)
									}
									className="flex items-center gap-3 rounded-lg border border-border/60 bg-background px-3 py-3 text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
								>
									<ProviderIcon
										id={provider.id}
										name={provider.name}
										size="sm"
									/>
									<div className="min-w-0 flex-1">
										<div className="truncate text-sm font-medium">
											{provider.name}
										</div>
										<div className="mt-0.5 text-xs text-muted-foreground">
											{count} model{count === 1 ? "" : "s"}
											{isConnected ? " · Connected" : ""}
										</div>
									</div>
									{isConnected && (
										<CheckIcon
											className="size-3.5 shrink-0 stroke-[1.5] text-emerald-600"
											aria-hidden="true"
										/>
									)}
								</button>
							)
						})
					)}
					{!catalog.loading && providers.length === 0 && (
						<p className="col-span-full py-8 text-center text-sm text-muted-foreground">
							{search.trim()
								? "No providers or models match your search."
								: "No providers available. Refresh the catalog to try again."}
						</p>
					)}
				</div>
			</div>
			<div className="space-y-3 text-center">
				<Button
					size="lg"
					className="min-w-40"
					onClick={() =>
						connectedCount > 0 ? onComplete(connectedCount) : onSkip()
					}
				>
					{connectedCount > 0 ? "Continue" : "I'll do this later"}
				</Button>
				{connectedCount === 0 && (
					<p className="text-xs text-muted-foreground">
						You can connect a provider later in Settings.
					</p>
				)}
			</div>
			{connection &&
				(isDesktopOAuthProvider(connection.id) ? (
					<ConnectProviderDialog
						provider={oauthProvider}
						onClose={() => setConnection(null)}
						onConnected={connected}
					/>
				) : (
					<TemplateConnectDialog
						provider={connection}
						open
						onOpenChange={(open) => {
							if (!open) setConnection(null)
						}}
						onConnected={connected}
					/>
				))}
			{detail && (
				<ConnectionDetailDialog
					provider={detail}
					connectionModels={catalog.data?.connectionModels[detail.id] ?? {}}
					open
					onOpenChange={(open) => {
						if (!open) setDetailId(null)
					}}
					onChanged={changed}
				/>
			)}
		</div>
	)
}
