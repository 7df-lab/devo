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
	type CatalogProviderInfo,
	type ModelRef,
	useProviderCatalog,
	useProviders,
} from "../../hooks/use-devo-data"
import { useModelDirectory } from "../../hooks/use-model-directory"
import { invalidateProviderDependentQueries } from "../../lib/invalidate-provider-queries"
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
import { getVariantTriggerLabel, resolveSelectedVariant } from "./model-selector-variant-label"

export interface ModelSelectorProps {
	directory: string | null
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
	directory,
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
	const availability = useProviders(directory)
	const catalog = useProviderCatalog()
	const modelDirectory = useModelDirectory()
	const navigate = useNavigate()
	const isMobile = useIsMobile()
	const models = useMemo(
		() =>
			buildPickerModels(
				availability.loading || availability.error ? null : availability.data,
				catalog.data,
				modelDirectory.data,
			),
		[
			availability.data,
			availability.loading,
			availability.error,
			catalog.data,
			modelDirectory.data,
		],
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
	const [connection, setConnection] = useState<CatalogProviderInfo | null>(null)
	// Catalog refetches must not restart an in-progress OAuth dialog.
	const oauthProvider = useMemo(
		() => (connection ? { ...connection, env: [] } : null),
		[connection],
	)
	const open = presentation === "inline" || (controlledOpen ?? localOpen)
	useEffect(() => {
		if (open) modelDirectory.refreshCatalog("ifStale")
	}, [open, modelDirectory.refreshCatalog])
	const changeOpen = (next: boolean) => {
		setLocalOpen(next)
		onOpenChange?.(next)
		if (!next) setMobileVariantView(false)
	}
	const unavailableReason =
		directory === null
			? "Choose a project to load available models"
			: !availability.connected
				? "Connect to the server to load available models"
				: availability.loading
					? "Loading available models…"
					: availability.error
						? "Could not load model availability"
						: null
	const choose = (model: PickerModel) => {
		if (unavailableReason) return
		if (model.ready) {
			onSelectModel(model.selection)
			changeOpen(false)
			return
		}
		const provider = catalog.data?.providers.find((entry) => entry.id === model.providerID)
		if (provider) {
			changeOpen(false)
			setConnection(provider)
		}
	}
	const connected = () => {
		invalidateProviderDependentQueries()
		catalog.reload()
		setConnection(null)
		changeOpen(true)
	}

	const content = (
		<>
			{mobileVariantView && variants.length && !unavailableReason ? (
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
						loading={availability.loading || catalog.loading || modelDirectory.loading}
						unavailableReason={unavailableReason}
						error={availability.error || catalog.error || modelDirectory.error}
						onRetry={() => {
							if (availability.error) availability.reload()
							else modelDirectory.refreshCatalog("force")
						}}
						onSelect={choose}
						refreshing={modelDirectory.refreshing}
						offline={modelDirectory.offline}
						onRefresh={() => modelDirectory.refreshCatalog("force")}
						onManageProviders={() => {
							changeOpen(false)
							if (onManageProviders) onManageProviders()
							else void navigate({ to: "/settings/providers" })
						}}
					/>
					{variants.length > 0 && !unavailableReason && (
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
									variantLabel={variants.length ? getVariantTriggerLabel(resolvedVariant) : null}
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
					<div className="overflow-hidden rounded-xl border border-border/60">{content}</div>
				)}
			</SearchableListPopover>
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
						onOpenChange={(next) => {
							if (!next) setConnection(null)
						}}
						onConnected={connected}
					/>
				))}
		</>
	)
}
