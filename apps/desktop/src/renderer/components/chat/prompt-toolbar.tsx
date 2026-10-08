import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Separator } from "@devo/ui/components/separator"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import { useAtomValue } from "jotai"
import {
	GitBranchIcon,
	MonitorIcon,
} from "lucide-react"
import { useCallback, useMemo } from "react"
import { itemsFamily } from "../../atoms/messages"
import type {
	CompactionConfig,
	ModelRef,
	ProvidersData,
	SdkAgent,
	VcsData,
} from "../../hooks/use-devo-data"
import {
	getModelCurrentVariant,
	getModelVariants,
	modelAllowsDefaultVariant,
} from "../../hooks/use-devo-data"
import {
	computeContextUsage,
	formatPercentage,
	type ModelLimitInfo,
	shortModelName,
} from "../../lib/session-metrics"
import { ModelSelector } from "./model-selector"
export { ModelSelector } from "./model-selector"

// ============================================================
// Shared toolbar trigger styles
// ============================================================

/**
 * Classes for SelectTrigger overrides. Uses `!` modifier to beat the base
 * component's `py-2 pl-2.5 pr-2 dark:bg-input/30 dark:hover:bg-input/50`.
 */
const TOOLBAR_TRIGGER_CN =
	"h-7! gap-1 border-none bg-transparent! hover:bg-muted! px-2! py-0! text-[13px] font-normal text-muted-foreground shadow-none transition-colors"

// ============================================================
// Agent Selector
// ============================================================

interface AgentSelectorProps {
	agents: SdkAgent[]
	selectedAgent: string | null
	defaultAgent?: string
	onSelectAgent: (agentName: string) => void
	disabled?: boolean
}

export function AgentSelector({
	agents,
	selectedAgent,
	defaultAgent,
	onSelectAgent,
	disabled,
}: AgentSelectorProps) {
	if (agents.length === 0) return null

	// Resolve which agent to display. If the preferred name doesn't match any
	// available agent (e.g. stale session data, config reload), fall back to a
	// known-valid agent so the Radix Select always has a matching SelectItem.
	const preferred = selectedAgent ?? defaultAgent ?? agents[0]?.name ?? "build"
	const currentAgentObj =
		agents.find((a) => a.name === preferred) ??
		agents.find((a) => a.name === defaultAgent) ??
		agents[0]
	const currentAgent = currentAgentObj?.name ?? preferred

	return (
		<Select
			value={currentAgent}
			onValueChange={(v) => {
				if (v !== null) onSelectAgent(v)
			}}
			disabled={disabled}
		>
			<SelectTrigger className={TOOLBAR_TRIGGER_CN}>
				<span className="flex items-center gap-1.5">
					{currentAgentObj?.color && (
						<span
							className="inline-block size-2 rounded-full"
							style={{ backgroundColor: currentAgentObj.color }}
						/>
					)}
					<span className="capitalize">{currentAgent}</span>
				</span>
			</SelectTrigger>
			<SelectContent side="top" align="start" alignItemWithTrigger={false}>
				{agents.map((agent) => (
					<SelectItem key={agent.name} value={agent.name}>
						<div className="flex items-center gap-2">
							{agent.color && (
								<span
									className="inline-block size-2 rounded-full"
									style={{ backgroundColor: agent.color }}
								/>
							)}
							<span className="capitalize">{agent.name}</span>
						</div>
					</SelectItem>
				))}
			</SelectContent>
		</Select>
	)
}

// ============================================================
// Model selector lives in model-selector.tsx.
// ============================================================

// ============================================================
// Variant Selector
// ============================================================

interface VariantSelectorProps {
	/** Available variant names for the current model */
	variants: string[]
	/** Currently selected variant (undefined = model default) */
	selectedVariant: string | undefined
	onSelectVariant: (variant: string | undefined) => void
	allowDefaultVariant?: boolean
	disabled?: boolean
}

export function VariantSelector({
	variants,
	selectedVariant,
	onSelectVariant,
	allowDefaultVariant = true,
	disabled,
}: VariantSelectorProps) {
	// Base UI Select needs an explicit items map so SelectValue can resolve
	// labels before the popup is opened (items only mount inside the portal).
	const items = useMemo(() => {
		const map: Record<string, string> = allowDefaultVariant
			? { __default__: "Default variant" }
			: {}
		for (const v of variants) {
			map[v] = v.charAt(0).toUpperCase() + v.slice(1)
		}
		return map
	}, [allowDefaultVariant, variants])

	if (variants.length === 0) return null

	// "default" is a sentinel for "no variant override".
	// If the selected variant isn't in the available list (e.g. stale restore),
	// fall back to the default sentinel when allowed, otherwise use the first
	// concrete option so reasoning-effort controls never show "Default variant".
	const value =
		selectedVariant && variants.includes(selectedVariant)
			? selectedVariant
			: allowDefaultVariant
				? "__default__"
				: variants[0]

	return (
		<Select
			value={value}
			onValueChange={(v) =>
				onSelectVariant(allowDefaultVariant && v === "__default__" ? undefined : (v ?? undefined))
			}
			disabled={disabled}
			items={items}
		>
			<SelectTrigger className={TOOLBAR_TRIGGER_CN}>
				<SelectValue />
			</SelectTrigger>
			<SelectContent
				side="top"
				align="start"
				alignItemWithTrigger={false}
				className="min-w-[160px]"
			>
				{allowDefaultVariant && (
					<SelectItem value="__default__">
						<span className="text-muted-foreground">Default variant</span>
					</SelectItem>
				)}
				{variants.map((variant) => (
					<SelectItem key={variant} value={variant}>
						<span className="capitalize">{variant}</span>
					</SelectItem>
				))}
			</SelectContent>
		</Select>
	)
}

// ============================================================
// Combined Prompt Toolbar
// ============================================================

export interface PromptToolbarProps {
	/** Available agents from Devo */
	agents: SdkAgent[]
	/** Currently selected agent name */
	selectedAgent: string | null
	/** Default agent from config */
	defaultAgent?: string
	onSelectAgent: (agentName: string) => void

	/** Provider data for model selector */
	providers: ProvidersData | null
	/** The resolved effective model */
	effectiveModel: ModelRef | null
	/** Whether the user has explicitly overridden the model */
	hasModelOverride: boolean
	onSelectModel: (model: ModelRef | null) => void
	modelPickerOpen?: boolean
	onModelPickerOpenChange?: (open: boolean) => void

	/** Currently selected variant */
	selectedVariant: string | undefined
	onSelectVariant: (variant: string | undefined) => void

	disabled?: boolean
}

/**
 * Combined toolbar with agent, model, and variant selectors.
 * Renders inside the PromptInputFooter > PromptInputTools slot.
 */
export function PromptToolbar({
	agents,
	selectedAgent,
	defaultAgent,
	onSelectAgent,
	providers,
	effectiveModel,
	hasModelOverride,
	onSelectModel,
	modelPickerOpen,
	onModelPickerOpenChange,
	selectedVariant,
	onSelectVariant,
	disabled,
}: PromptToolbarProps) {
	// Compute variants for the current effective model
	const variants = useMemo(() => {
		if (!effectiveModel || !providers) return []
		return getModelVariants(effectiveModel.providerID, effectiveModel.modelID, providers.providers)
	}, [effectiveModel, providers])
	const currentVariant = useMemo(() => {
		if (!effectiveModel || !providers) return undefined
		return getModelCurrentVariant(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
	}, [effectiveModel, providers])
	const allowDefaultVariant = useMemo(() => {
		if (!effectiveModel || !providers) return true
		return modelAllowsDefaultVariant(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
	}, [effectiveModel, providers])

	const hasAgents = agents.length > 0

	return (
		<div className="flex min-w-0 flex-wrap items-center gap-0.5">
			{hasAgents && (
				<AgentSelector
					agents={agents}
					selectedAgent={selectedAgent}
					defaultAgent={defaultAgent}
					onSelectAgent={onSelectAgent}
					disabled={disabled}
				/>
			)}

			{hasAgents && <Separator orientation="vertical" className="mx-0.5 my-2 self-stretch" />}

			<ModelSelector
				providers={providers}
				effectiveModel={effectiveModel}
				hasOverride={hasModelOverride}
				onSelectModel={onSelectModel}
				open={modelPickerOpen}
				onOpenChange={onModelPickerOpenChange}
				variants={variants}
				selectedVariant={selectedVariant}
				currentVariant={currentVariant}
				allowDefaultVariant={allowDefaultVariant}
				onSelectVariant={onSelectVariant}
				disabled={disabled}
			/>
		</div>
	)
}

// ============================================================
// Status Bar (below the input card)
// ============================================================

interface StatusBarProps {
	vcs: VcsData | null
	isConnected: boolean
	/** Whether the session is currently running */
	isWorking?: boolean
	/** Number of Escape presses toward abort (0 = none, 1 = first press) */
	interruptCount?: number
	/** Optional slot to replace the default branch display (e.g. interactive BranchPicker) */
	branchSlot?: React.ReactNode
	/** Optional extra slot rendered on the left side (e.g. worktree toggle) */
	extraSlot?: React.ReactNode
	/** Session ID for context usage computation */
	sessionId?: string
	/** Provider data for context limit lookup */
	providers?: ProvidersData | null
	/** Compaction config from Devo for accurate threshold calculation */
	compaction?: CompactionConfig
}

export function StatusBar({
	vcs,
	isConnected,
	isWorking,
	interruptCount,
	branchSlot,
	extraSlot,
	sessionId,
	providers,
	compaction,
}: StatusBarProps) {
	return (
		<div className="flex min-w-0 items-center gap-3 overflow-hidden px-2 pt-2 text-[11px] text-muted-foreground/60">
			{/* Left side — environment + connection + interrupt hint */}
			<div className="flex shrink-0 items-center gap-3">
				{extraSlot ?? (
					<div className="flex items-center gap-1">
						<MonitorIcon className="size-3" />
						<span>Local</span>
					</div>
				)}

				{!isConnected && (
					<div className="flex items-center gap-1 text-yellow-500/70">
						<span className="inline-block size-1.5 rounded-full bg-yellow-500/70" />
						<span>Disconnected</span>
					</div>
				)}

				{/* Escape-to-abort hint — shown when session is working */}
				{isConnected && isWorking && (
					<div
						className={`flex items-center gap-1 transition-colors ${interruptCount && interruptCount > 0 ? "text-orange-400" : ""}`}
					>
						<kbd className="rounded border border-border px-1 py-0.5 font-mono text-[10px] leading-none">
							esc
						</kbd>
						<span>
							{interruptCount && interruptCount > 0 ? "press again to stop" : "interrupt"}
						</span>
					</div>
				)}
			</div>

			{/* Right side — context usage + git branch */}
			<div className="ml-auto flex min-w-0 items-center gap-3 overflow-hidden">
				{/* Context window usage */}
				{sessionId && (
					<ContextUsageIndicator
						sessionId={sessionId}
						providers={providers}
						compaction={compaction}
					/>
				)}

				{/* Git branch — interactive picker or read-only display */}
				{branchSlot
					? branchSlot
					: vcs?.branch && (
							<div className="flex items-center gap-1">
								<GitBranchIcon className="size-3" />
								<span className="max-w-[140px] truncate">{vcs.branch}</span>
							</div>
						)}
			</div>
		</div>
	)
}

// ============================================================
// Context window usage indicator (for StatusBar)
// ============================================================

/**
 * Compact context usage indicator: circular progress + percentage.
 * Reads messages from the Jotai atom and computes context window usage
 * against the model's context limit from provider data.
 *
 * Renders nothing when there are no assistant messages with token data,
 * or when provider data is unavailable for the current model.
 */
function ContextUsageIndicator({
	sessionId,
	providers,
	compaction,
}: {
	sessionId: string
	providers?: ProvidersData | null
	compaction?: CompactionConfig
}) {
	const messages = useAtomValue(itemsFamily(sessionId))

	const getModelLimit = useCallback(
		(providerID: string, modelID: string): ModelLimitInfo | undefined => {
			if (!providers?.providers) return undefined
			for (const provider of providers.providers) {
				if (provider.id !== providerID) continue
				const model = provider.models[modelID]
				if (model?.limit?.context) return model.limit
			}
			return undefined
		},
		[providers],
	)

	const compactionOptions = useMemo(
		() => (compaction ? { auto: compaction.auto, reserved: compaction.reserved } : undefined),
		[compaction],
	)

	const usage = useMemo(
		() => computeContextUsage(messages, getModelLimit, compactionOptions),
		[messages, getModelLimit, compactionOptions],
	)

	if (!usage) return null

	const pct = usage.percentage
	const color = pct >= 90 ? "text-red-400" : pct >= 70 ? "text-yellow-400" : ""

	const compPct = usage.compactionPercentage
	const compColor =
		compPct != null && compPct >= 100
			? "text-red-400"
			: compPct != null && compPct >= 80
				? "text-yellow-400"
				: "text-background/60"

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<span
						className={cn(
							"inline-flex items-center gap-1 tabular-nums transition-colors hover:text-foreground",
							color,
						)}
					/>
				}
			>
				<ContextCircle percentage={pct} size={12} strokeWidth={1.5} />
				<span>{formatPercentage(pct)}</span>
			</TooltipTrigger>
			<TooltipContent side="top" align="end">
				<div className="space-y-1.5 text-xs">
					<p className="font-medium">Context Window</p>
					<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-background/60">
						<span>Usage</span>
						<span className="text-right tabular-nums">{formatPercentage(pct)}</span>
						<span>Tokens</span>
						<span className="text-right tabular-nums">
							{usage.lastMessageTokens.toLocaleString()}
						</span>
						<span>Limit</span>
						<span className="text-right tabular-nums">{usage.contextLimit.toLocaleString()}</span>
						<span>Model</span>
						<span className="text-right">{shortModelName(usage.modelID)}</span>
					</div>
					{usage.compactionThreshold != null && compPct != null && (
						<div className="border-t border-background/15 pt-1">
							<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-background/60">
								<span>Compaction</span>
								<span className={cn("text-right tabular-nums", compColor)}>
									{compPct >= 100 ? "now" : `at ${usage.compactionThreshold.toLocaleString()}`}
								</span>
								<span>Remaining</span>
								<span className={cn("text-right tabular-nums", compColor)}>
									{compPct >= 100
										? "overflowed"
										: `${(usage.compactionThreshold - usage.lastMessageTokens).toLocaleString()} tokens`}
								</span>
							</div>
						</div>
					)}
				</div>
			</TooltipContent>
		</Tooltip>
	)
}

// ============================================================
// SVG circular progress
// ============================================================

function ContextCircle({
	percentage,
	size = 12,
	strokeWidth = 1.5,
}: {
	percentage: number
	size?: number
	strokeWidth?: number
}) {
	const radius = (size - strokeWidth) / 2
	const circumference = 2 * Math.PI * radius
	const offset = circumference - (Math.min(percentage, 100) / 100) * circumference

	const strokeColor =
		percentage >= 90 ? "stroke-red-400" : percentage >= 70 ? "stroke-yellow-400" : "stroke-current"

	return (
		<svg
			width={size}
			height={size}
			viewBox={`0 0 ${size} ${size}`}
			className="shrink-0"
			aria-hidden="true"
		>
			<circle
				cx={size / 2}
				cy={size / 2}
				r={radius}
				fill="none"
				className="stroke-muted-foreground/15"
				strokeWidth={strokeWidth}
			/>
			<circle
				cx={size / 2}
				cy={size / 2}
				r={radius}
				fill="none"
				className={strokeColor}
				strokeWidth={strokeWidth}
				strokeDasharray={circumference}
				strokeDashoffset={offset}
				strokeLinecap="round"
				transform={`rotate(-90 ${size / 2} ${size / 2})`}
			/>
		</svg>
	)
}
