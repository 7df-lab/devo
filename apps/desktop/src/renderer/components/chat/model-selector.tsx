import {
	SearchableListPopover,
	SearchableListPopoverContent,
	SearchableListPopoverTrigger,
} from "@devo/ui/components/searchable-list-popover"
import { useIsMobile } from "@devo/ui/hooks/use-mobile"
import { useNavigate } from "@tanstack/react-router"
import { ChevronDownIcon } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import {
	useProviderCatalog,
	type CatalogProviderInfo,
	type ModelRef,
	type ProvidersData,
} from "../../hooks/use-devo-data"
import { invalidateProviderDependentQueries } from "../../lib/invalidate-provider-queries"
import { useModelDirectory } from "../../hooks/use-model-directory"
import { ConnectProviderDialog } from "../settings/connect-provider-dialog"
import { isDesktopOAuthProvider } from "../settings/desktop-oauth-providers"
import { TemplateConnectDialog } from "../settings/template-connect-dialog"
import { buildPickerModels, type PickerModel } from "./model-picker-data"
import { ModelPickerList } from "./model-picker-list"
import {
	ModelSelectorReasoningStrength,
	ModelSelectorReasoningStrengthMobileView,
} from "./model-selector-reasoning-strength"
import { ModelSelectorTriggerLabel } from "./model-selector-trigger-label"
import {
	getVariantTriggerLabel,
	resolveSelectedVariant,
} from "./model-selector-variant-label"

export interface ModelSelectorProps {
	providers: ProvidersData | null
	effectiveModel: ModelRef | null
	hasOverride: boolean
	onSelectModel: (model: ModelRef | null) => void
	variants?: string[]
	selectedVariant?: string
	currentVariant?: string
	allowDefaultVariant?: boolean
	onSelectVariant?: (variant: string | undefined) => void
	disabled?: boolean
	presentation?: "popover" | "inline"
	onManageProviders?: () => void
	open?: boolean
	onOpenChange?: (open: boolean) => void
}

/** Shared desktop equivalent of /model, backed by Native preferences and catalog. */
export function ModelSelector({
	providers,
	effectiveModel,
	onSelectModel,
	variants = [],
	selectedVariant,
	currentVariant,
	allowDefaultVariant = true,
	onSelectVariant = () => undefined,
	disabled,
	presentation = "popover",
	onManageProviders,
	open: controlledOpen,
	onOpenChange,
}: ModelSelectorProps) {
	const catalog = useProviderCatalog()
	const directory = useModelDirectory()
	const navigate = useNavigate()
	const isMobile = useIsMobile()
	const models = useMemo(
		() => buildPickerModels(providers, catalog.data, directory.data),
		[providers, catalog.data, directory.data],
	)
	const activeValue = effectiveModel
		? effectiveModel.providerID === "session"
			? effectiveModel.modelID
			: `${effectiveModel.providerID}/${effectiveModel.modelID}`
		: null
	const activeModel = models.find((model) => model.value === activeValue)
	const resolvedVariant = resolveSelectedVariant(
		variants,
		selectedVariant,
		currentVariant,
		allowDefaultVariant,
	)
	const [localOpen, setLocalOpen] = useState(false)
	const [mobileVariantView, setMobileVariantView] = useState(false)
	const [connection, setConnection] = useState<{
		provider: CatalogProviderInfo
		model: PickerModel
	} | null>(null)
	// Catalog refetches must not restart an in-progress OAuth dialog.
	const oauthProvider = useMemo(
		() => (connection ? { ...connection.provider, env: [] } : null),
		[connection],
	)
	const open = presentation === "inline" || (controlledOpen ?? localOpen)
	useEffect(() => {
		if (open) directory.refreshCatalog("ifStale")
	}, [open, directory.refreshCatalog])
	const changeOpen = (next: boolean) => {
		setLocalOpen(next)
		onOpenChange?.(next)
		if (!next) setMobileVariantView(false)
	}
	const choose = (model: PickerModel) => {
		if (model.ready) {
			onSelectModel(model.selection)
			changeOpen(false)
			return
		}
		const provider = catalog.data?.providers.find(
			(entry) => entry.id === model.providerID,
		)
		if (provider) {
			changeOpen(false)
			setConnection({ provider, model })
		}
	}
	const connected = () => {
		invalidateProviderDependentQueries()
		catalog.reload()
		if (connection) onSelectModel(connection.model.selection)
		setConnection(null)
	}

	const content = (
		<>
			{mobileVariantView && variants.length ? (
				<ModelSelectorReasoningStrengthMobileView
					variants={variants}
					selectedVariant={resolvedVariant}
					allowDefaultVariant={allowDefaultVariant}
					onBack={() => setMobileVariantView(false)}
					onSelectVariant={onSelectVariant}
					onClose={() => changeOpen(false)}
				/>
			) : (
				<>
					<ModelPickerList
						models={models}
						activeValue={activeValue}
						loading={catalog.loading || directory.loading}
						error={catalog.error || directory.error}
						onRetry={() => directory.refreshCatalog("force")}
						onSelect={choose}
						refreshing={directory.refreshing}
						offline={directory.offline}
						onRefresh={() => directory.refreshCatalog("force")}
						onManageProviders={() => {
							changeOpen(false)
							if (onManageProviders) onManageProviders()
							else void navigate({ to: "/settings/providers" })
						}}
					/>
					{variants.length > 0 && (
						<ModelSelectorReasoningStrength
							variants={variants}
							selectedVariant={resolvedVariant}
							allowDefaultVariant={allowDefaultVariant}
							isMobile={isMobile}
							onOpenMobileView={() => setMobileVariantView(true)}
							onSelectVariant={onSelectVariant}
							onClose={() => changeOpen(false)}
						/>
					)}
				</>
			)}
		</>
	)

	return (
		<>
			<SearchableListPopover open={open} onOpenChange={changeOpen}>
				{presentation === "popover" ? (
					<>
						<SearchableListPopoverTrigger
							disabled={disabled}
							aria-label="Choose model"
							title="Choose model · /model"
							className="flex h-7 items-center gap-1 rounded-md border-none bg-transparent px-2 text-[13px] font-normal shadow-none transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
						>
							{activeModel ? (
								<ModelSelectorTriggerLabel
									displayName={activeModel.displayName}
									variantLabel={
										variants.length
											? getVariantTriggerLabel(resolvedVariant)
											: null
									}
								/>
							) : (
								<span className="text-muted-foreground">Choose model</span>
							)}
							<ChevronDownIcon className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/50" />
						</SearchableListPopoverTrigger>
						<SearchableListPopoverContent
							side="top"
							align="end"
							width="w-[min(360px,calc(100vw-24px))]"
							aria-label="Choose model"
						>
							{content}
						</SearchableListPopoverContent>
					</>
				) : (
					<div className="overflow-hidden rounded-xl border border-border/60">
						{content}
					</div>
				)}
			</SearchableListPopover>
			{connection &&
				(isDesktopOAuthProvider(connection.provider.id) ? (
					<ConnectProviderDialog
						provider={oauthProvider}
						onClose={() => setConnection(null)}
						onConnected={connected}
					/>
				) : (
					<TemplateConnectDialog
						provider={connection.provider}
						open
						onOpenChange={(next) => {
							if (!next) setConnection(null)
						}}
						onConnected={connected}
					/>
				))}
		</>
	)
}
