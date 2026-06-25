/**
 * Provider management settings tab.
 * Shows connected providers and allows connecting/disconnecting AI providers.
 *
 * Uses two endpoints:
 * - GET /provider/ (full catalog + connected IDs)
 * - GET /config/providers (connected providers with source field)
 *
 * The source field tells us HOW a provider is connected:
 * - "env"    -- via environment variable (e.g. ANTHROPIC_API_KEY)
 * - "api"    -- via API key stored in auth.json
 * - "custom" -- via OAuth or plugin (e.g. Claude Pro/Max)
 * - "config" -- via devo.json config (overrides other sources)
 *
 * Key UX decisions:
 * - Devo Zen: always sorted first, shows free/paid tier indicator
 * - Env-only providers: show "Environment" badge, no disconnect
 * - OAuth/API key providers: show source badge + disconnect button
 * - "Browse all providers" button opens a dialog with search + full catalog
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { ScrollArea } from "@devo/ui/components/scroll-area"
import { Skeleton } from "@devo/ui/components/skeleton"
import {
	Tooltip,
	TooltipContent,
	TooltipProvider,
	TooltipTrigger,
} from "@devo/ui/components/tooltip"
import { useQueryClient } from "@tanstack/react-query"
import {
	AlertCircleIcon,
	CheckIcon,
	ExternalLinkIcon,
	GridIcon,
	LinkIcon,
	SearchIcon,
	UnlinkIcon,
	ZapIcon,
} from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import type {
	ConnectedProviderInfo,
	SdkProviderAuthMethod as ProviderAuthMethod,
} from "../../hooks/use-devo-data"
import {
	type CatalogProvider,
	queryKeys,
	useAllProviders,
	useConnectedProviders,
	useProviderAuthMethods,
} from "../../hooks/use-devo-data"
import { createLogger } from "../../lib/logger"
import {
	compareByPopularity,
	isSubscriptionConnected,
	isZenFreeTier,
	POPULAR_PROVIDER_IDS,
	PROVIDER_KEY_URLS,
	SUBSCRIPTION_LABELS,
	ZEN_PROVIDER_ID,
	ZEN_SIGNUP_URL,
} from "../../lib/providers"
import { getBaseClient } from "../../services/connection-manager"
import { ConnectProviderDialog } from "./connect-provider-dialog"
import { ProviderIcon } from "./provider-icon"
import { SettingsSection } from "./settings-section"

const log = createLogger("provider-settings")

// ============================================================
// Constants
// ============================================================

/** Human-readable labels for source types */
const SOURCE_LABELS: Record<string, string> = {
	env: "Environment",
	api: "API Key",
	custom: "OAuth",
	config: "Config",
}

/**
 * Derive a short auth method summary string for a provider.
 * Uses plugin-provided auth methods when available, otherwise infers from env array.
 */
function getAuthSummary(methods: ProviderAuthMethod[] | null, env: string[]): string | null {
	if (methods && methods.length > 0) {
		// Deduplicate by type, keep labels for OAuth methods
		const hasOAuth = methods.some((m) => m.type === "oauth")
		const hasApi = methods.some((m) => m.type === "api")
		const parts: string[] = []
		if (hasOAuth) parts.push("OAuth")
		if (hasApi) parts.push("API Key")
		return parts.join(", ")
	}

	// No plugin methods: infer from env array
	if (env.length > 0) return "API Key"
	return null
}

// ============================================================
// Main component
// ============================================================

export function ProviderSettings() {
	const {
		data: allProviders,
		loading: catalogLoading,
		error,
		reload: reloadCatalog,
	} = useAllProviders()
	const {
		data: connectedInfo,
		loading: connectedLoading,
		reload: reloadConnected,
	} = useConnectedProviders()
	const { data: authMethods } = useProviderAuthMethods()
	const [connectDialogProvider, setConnectDialogProvider] = useState<CatalogProvider | null>(null)
	const [catalogOpen, setCatalogOpen] = useState(false)

	const loading = catalogLoading || connectedLoading

	const reload = useCallback(() => {
		reloadCatalog()
		reloadConnected()
	}, [reloadCatalog, reloadConnected])

	if (loading) {
		return <ProviderSettingsLoading />
	}

	if (error) {
		return (
			<div className="space-y-8">
				<ProviderSettingsHeader />
				<div className="flex items-center gap-3 rounded-lg border border-destructive/50 bg-destructive/10 px-4 py-3 text-sm text-destructive">
					<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
					<span>Failed to load providers: {error}</span>
					<Button variant="outline" size="sm" className="ml-auto" onClick={reload}>
						Retry
					</Button>
				</div>
			</div>
		)
	}

	if (!allProviders) return null

	const connectedSet = new Set(allProviders.connected)

	const connectedProviders = allProviders.all
		.filter((p) => connectedSet.has(p.id))
		.sort(compareByPopularity)

	const popularUnconnected = allProviders.all
		.filter(
			(p) =>
				POPULAR_PROVIDER_IDS.includes(p.id as (typeof POPULAR_PROVIDER_IDS)[number]) &&
				!connectedSet.has(p.id),
		)
		.sort(compareByPopularity)

	return (
		<div className="space-y-8">
			<ProviderSettingsHeader />

			{connectedProviders.length > 0 && (
				<SettingsSection title="Connected">
					{connectedProviders.map((provider) => (
						<ConnectedProviderRow
							key={provider.id}
							provider={provider}
							sourceInfo={connectedInfo?.get(provider.id) ?? null}
							onConnect={() => setConnectDialogProvider(provider)}
							onReload={reload}
						/>
					))}
				</SettingsSection>
			)}

			{popularUnconnected.length > 0 && (
				<SettingsSection title="Available">
					{popularUnconnected.map((provider) => (
						<AvailableProviderRow
							key={provider.id}
							provider={provider}
							onConnect={() => setConnectDialogProvider(provider)}
						/>
					))}
				</SettingsSection>
			)}

			<div className="sticky bottom-6">
				<Button
					variant="outline"
					className="w-full backdrop-blur-md bg-background/70"
					onClick={() => setCatalogOpen(true)}
				>
					<GridIcon className="size-4" aria-hidden="true" />
					Browse all {allProviders.all.length} providers
				</Button>
			</div>

			{/* All providers dialog */}
			<AllProvidersDialog
				open={catalogOpen}
				onOpenChange={setCatalogOpen}
				allProviders={allProviders.all}
				connectedIds={connectedSet}
				connectedInfo={connectedInfo ?? null}
				authMethods={authMethods ?? null}
				onConnect={(provider) => {
					setCatalogOpen(false)
					setConnectDialogProvider(provider)
				}}
			/>

			<ConnectProviderDialog
				provider={connectDialogProvider}
				pluginAuthMethods={
					connectDialogProvider ? authMethods?.[connectDialogProvider.id] : undefined
				}
				onClose={() => setConnectDialogProvider(null)}
				onConnected={() => {
					setConnectDialogProvider(null)
					reload()
				}}
			/>
		</div>
	)
}

// ============================================================
// Sub-components
// ============================================================

function ProviderSettingsHeader() {
	return (
		<div>
			<h2 className="text-xl font-semibold">Providers</h2>
			<p className="text-sm text-muted-foreground mt-1">
				Connect AI providers to use their models.{" "}
				<a
					href="https://devo.ai/docs/providers/"
					target="_blank"
					rel="noopener noreferrer"
					className="text-primary hover:underline"
				>
					Learn more &rsaquo;
				</a>
			</p>
		</div>
	)
}

function ConnectedProviderRow({
	provider,
	sourceInfo,
	onConnect,
	onReload,
}: {
	provider: CatalogProvider
	sourceInfo: ConnectedProviderInfo | null
	onConnect: () => void
	onReload: () => void
}) {
	const [disconnecting, setDisconnecting] = useState(false)
	const queryClient = useQueryClient()

	const source = sourceInfo?.source ?? "api"
	const isEnvConnected = source === "env"
	const hasManualAuth = source === "api" || source === "custom"

	const isZen = provider.id === ZEN_PROVIDER_ID
	const zenFree = isZen && isZenFreeTier(provider.models)

	const handleRemoveAuth = useCallback(async () => {
		setDisconnecting(true)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			await client.auth.remove({ providerID: provider.id })
			await client.global.dispose()
			queryClient.invalidateQueries({ queryKey: queryKeys.allProviders })
			queryClient.invalidateQueries({ queryKey: queryKeys.connectedProviders })
			queryClient.invalidateQueries({
				predicate: (q) => q.queryKey[0] === "providers",
			})
			onReload()
		} catch (err) {
			log.error("Failed to disconnect provider", { provider: provider.id, error: err })
		} finally {
			setDisconnecting(false)
		}
	}, [provider.id, queryClient, onReload])

	const modelCount = Object.keys(provider.models).length

	// Detect subscription plans (e.g. Claude Pro/Max, ChatGPT Pro/Plus) via zeroed-out costs
	const isSubscription = isSubscriptionConnected(provider.models)
	const subscriptionLabel = SUBSCRIPTION_LABELS[provider.id]

	// Subscription users can always disconnect (remove OAuth tokens from auth.json)
	const canDisconnect = hasManualAuth || isSubscription

	const sourceLabel =
		isSubscription && subscriptionLabel ? subscriptionLabel : (SOURCE_LABELS[source] ?? source)

	// Hide the "Get API key" link for subscription-connected providers
	const keyUrl = isSubscription ? undefined : PROVIDER_KEY_URLS[provider.id]

	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<ProviderIcon id={provider.id} name={provider.name} />
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<div className="flex items-center gap-2">
					<span className="text-sm font-medium">{provider.name}</span>
					{zenFree && (
						<span className="inline-flex items-center gap-0.5 rounded-full bg-emerald-500/10 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-emerald-600 dark:text-emerald-400">
							<ZapIcon className="size-2" aria-hidden="true" />
							Free
						</span>
					)}
				</div>
				<div className="flex items-center gap-2">
					<span className="text-xs text-muted-foreground">
						{modelCount} {modelCount === 1 ? "model" : "models"}
					</span>
					{zenFree && (
						<>
							<span className="text-muted-foreground/30">|</span>
							<a
								href={ZEN_SIGNUP_URL}
								target="_blank"
								rel="noopener noreferrer"
								className="inline-flex items-center gap-1 text-[11px] text-primary hover:underline"
							>
								Upgrade
								<ExternalLinkIcon className="size-2.5" aria-hidden="true" />
							</a>
						</>
					)}
				</div>
			</div>
			<div className="flex items-center gap-2">
				<TooltipProvider>
					{zenFree ? (
						<Button variant="outline" size="sm" className="h-7 text-xs" onClick={onConnect}>
							Add API key
						</Button>
					) : (
						<>
							<Tooltip>
								<TooltipTrigger
									render={
										<Badge
											variant={isEnvConnected ? "secondary" : "outline"}
											className="cursor-default"
										/>
									}
								>
									{sourceLabel}
								</TooltipTrigger>
								<TooltipContent>
									<SourceTooltip
										source={source}
										envVars={provider.env}
										subscriptionLabel={isSubscription ? subscriptionLabel : undefined}
									/>
								</TooltipContent>
							</Tooltip>

							{keyUrl && (
								<Tooltip>
									<TooltipTrigger
										render={
											// biome-ignore lint/a11y/useAnchorContent: content provided via Base UI render prop
											<a
												aria-label="Get API key"
												href={keyUrl.url}
												target="_blank"
												rel="noopener noreferrer"
												className="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
											/>
										}
									>
										<ExternalLinkIcon className="size-3.5" aria-hidden="true" />
									</TooltipTrigger>
									<TooltipContent>
										<p>Manage keys on {new URL(keyUrl.url).hostname}</p>
									</TooltipContent>
								</Tooltip>
							)}

							{canDisconnect && (
								<Tooltip>
									<TooltipTrigger
										render={
											<Button
												variant="ghost"
												size="sm"
												onClick={handleRemoveAuth}
												disabled={disconnecting}
												className="text-muted-foreground hover:text-destructive"
											/>
										}
									>
										<UnlinkIcon className="size-4" aria-hidden="true" />
									</TooltipTrigger>
									<TooltipContent>
										<p>Disconnect {provider.name}</p>
									</TooltipContent>
								</Tooltip>
							)}
						</>
					)}
				</TooltipProvider>
			</div>
		</div>
	)
}

function SourceTooltip({
	source,
	envVars,
	subscriptionLabel,
}: {
	source: string
	envVars: string[]
	subscriptionLabel?: string
}) {
	if (subscriptionLabel) {
		return <p>Connected via {subscriptionLabel} subscription</p>
	}
	switch (source) {
		case "env":
			return (
				<p>
					Connected via environment variable
					{envVars.length > 0 && (
						<>
							{" "}
							(<code className="text-xs">{envVars[0]}</code>)
						</>
					)}
				</p>
			)
		case "api":
			return <p>Connected with an API key</p>
		case "custom":
			return <p>Connected via OAuth</p>
		case "config":
			return <p>Configured in devo.json</p>
		default:
			return <p>Connected</p>
	}
}

function AvailableProviderRow({
	provider,
	onConnect,
}: {
	provider: CatalogProvider
	onConnect: () => void
}) {
	const modelCount = Object.keys(provider.models).length
	const keyUrl = PROVIDER_KEY_URLS[provider.id]

	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<ProviderIcon id={provider.id} name={provider.name} />
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<span className="text-sm font-medium">{provider.name}</span>
				<span className="text-xs text-muted-foreground">
					{modelCount} {modelCount === 1 ? "model" : "models"}
				</span>
			</div>
			<div className="flex items-center gap-2">
				{keyUrl && (
					<a
						href={keyUrl.url}
						target="_blank"
						rel="noopener noreferrer"
						className="inline-flex items-center gap-1 text-[11px] text-muted-foreground transition-colors hover:text-primary"
					>
						<ExternalLinkIcon className="size-3" aria-hidden="true" />
						{keyUrl.label}
					</a>
				)}
				<Button
					variant="outline"
					size="sm"
					onClick={onConnect}
					aria-label={`Connect ${provider.name}`}
				>
					<LinkIcon className="size-3.5" aria-hidden="true" />
					Connect
				</Button>
			</div>
		</div>
	)
}

// ============================================================
// All Providers Dialog
// ============================================================

function AllProvidersDialog({
	open,
	onOpenChange,
	allProviders,
	connectedIds,
	connectedInfo,
	authMethods,
	onConnect,
}: {
	open: boolean
	onOpenChange: (open: boolean) => void
	allProviders: CatalogProvider[]
	connectedIds: Set<string>
	connectedInfo: Map<string, ConnectedProviderInfo> | null
	authMethods: Record<string, ProviderAuthMethod[]> | null
	onConnect: (provider: CatalogProvider) => void
}) {
	const [search, setSearch] = useState("")
	const searchRef = useRef<HTMLInputElement>(null)

	// Auto-focus search when dialog opens, clear search when it closes
	useEffect(() => {
		if (open) {
			const timer = setTimeout(() => searchRef.current?.focus(), 50)
			return () => clearTimeout(timer)
		}
		setSearch("")
	}, [open])

	const query = search.toLowerCase().trim()
	const filtered = allProviders
		.filter((p) => !query || p.name.toLowerCase().includes(query) || p.id.includes(query))
		.sort((a, b) => {
			// Connected providers first, then by popularity, then alphabetical
			const ac = connectedIds.has(a.id)
			const bc = connectedIds.has(b.id)
			if (ac !== bc) return ac ? -1 : 1
			return compareByPopularity(a, b)
		})

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="flex h-[70vh] max-h-[85vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-lg">
				<DialogHeader className="px-6 pt-6 pb-4">
					<DialogTitle>All Providers</DialogTitle>
					<DialogDescription>
						{allProviders.length} providers available. Connect one to start using its models.
					</DialogDescription>
				</DialogHeader>

				{/* Search bar */}
				<div className="border-y border-border px-4 py-2">
					<div className="relative">
						<SearchIcon
							className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground/50"
							aria-hidden="true"
						/>
						<Input
							ref={searchRef}
							value={search}
							onChange={(e) => setSearch(e.target.value)}
							placeholder="Search providers..."
							className="h-8 border-0 bg-transparent pl-8 shadow-none focus-visible:ring-0"
						/>
					</div>
				</div>

				{/* Provider list */}
				<ScrollArea className="min-h-0 flex-1 overflow-hidden">
					<div className="divide-y divide-border">
						{filtered.length === 0 ? (
							<div className="px-4 py-12 text-center text-sm text-muted-foreground">
								No providers matching &ldquo;{search}&rdquo;
							</div>
						) : (
							filtered.map((provider) => (
								<CatalogRow
									key={provider.id}
									provider={provider}
									isConnected={connectedIds.has(provider.id)}
									sourceInfo={connectedInfo?.get(provider.id) ?? null}
									authMethods={authMethods?.[provider.id] ?? null}
									onConnect={() => onConnect(provider)}
								/>
							))
						)}
					</div>
				</ScrollArea>
			</DialogContent>
		</Dialog>
	)
}

function CatalogRow({
	provider,
	isConnected,
	sourceInfo,
	authMethods,
	onConnect,
}: {
	provider: CatalogProvider
	isConnected: boolean
	sourceInfo: ConnectedProviderInfo | null
	authMethods: ProviderAuthMethod[] | null
	onConnect: () => void
}) {
	const modelCount = Object.keys(provider.models).length
	const source = sourceInfo?.source
	const isZen = provider.id === ZEN_PROVIDER_ID
	const isSubscription = isConnected && isSubscriptionConnected(provider.models)
	const subscriptionLabel = SUBSCRIPTION_LABELS[provider.id]

	// Derive the connected label
	const connectedLabel = isZen
		? "Active"
		: isSubscription && subscriptionLabel
			? subscriptionLabel
			: source
				? (SOURCE_LABELS[source] ?? "Connected")
				: "Connected"

	// Derive available auth method summary for unconnected providers
	const authSummary = getAuthSummary(authMethods, provider.env)

	return (
		<div className="flex items-center gap-3 px-4 py-2.5">
			<ProviderIcon id={provider.id} name={provider.name} size="sm" />
			<div className="flex min-w-0 flex-1 flex-col">
				<span className="text-sm font-medium">{provider.name}</span>
				<span className="text-xs text-muted-foreground">
					{modelCount} {modelCount === 1 ? "model" : "models"}
					{authSummary && (
						<>
							<span className="mx-1.5 text-muted-foreground/30">&middot;</span>
							{authSummary}
						</>
					)}
				</span>
			</div>
			{isConnected ? (
				<div className="flex items-center gap-1.5 text-xs font-medium text-emerald-600 dark:text-emerald-400">
					<CheckIcon className="size-3.5" aria-hidden="true" />
					<span>{connectedLabel}</span>
				</div>
			) : (
				<Button
					variant="ghost"
					size="sm"
					onClick={onConnect}
					className="h-7 text-xs"
					aria-label={`Connect ${provider.name}`}
				>
					Connect
				</Button>
			)}
		</div>
	)
}

// ============================================================
// Loading state
// ============================================================

function ProviderSettingsLoading() {
	return (
		<div className="space-y-8">
			<ProviderSettingsHeader />
			<SettingsSection title="Connected">
				{[1, 2].map((i) => (
					<div key={i} className="flex items-center gap-3 px-4 py-3">
						<Skeleton className="size-8 rounded-md" />
						<div className="flex flex-col gap-1.5">
							<Skeleton className="h-4 w-24" />
							<Skeleton className="h-3 w-16" />
						</div>
					</div>
				))}
			</SettingsSection>
		</div>
	)
}
