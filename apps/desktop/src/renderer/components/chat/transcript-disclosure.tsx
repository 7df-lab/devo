import {
	Collapsible,
	CollapsibleContent,
	CollapsibleTrigger,
} from "@devo/ui/components/collapsible"
import { cn } from "@devo/ui/lib/utils"
import { ChevronDownIcon, ChevronRightIcon } from "lucide-react"
import {
	createContext,
	memo,
	useCallback,
	useContext,
	useMemo,
	useState,
	type ReactNode,
} from "react"

interface TranscriptDisclosureContextValue {
	isOpen: boolean
	expandable: boolean
}

const TranscriptDisclosureContext = createContext<TranscriptDisclosureContextValue | null>(null)

function useTranscriptDisclosure() {
	const context = useContext(TranscriptDisclosureContext)
	if (!context) {
		throw new Error("Transcript disclosure components must be used within TranscriptDisclosure")
	}
	return context
}

export interface TranscriptDisclosureProps {
	open?: boolean
	defaultOpen?: boolean
	onOpenChange?: (open: boolean) => void
	expandable?: boolean
	forceOpen?: boolean
	className?: string
	children: ReactNode
}

export const TranscriptDisclosure = memo(function TranscriptDisclosure({
	open: openProp,
	defaultOpen = false,
	onOpenChange,
	expandable = true,
	forceOpen = false,
	className,
	children,
}: TranscriptDisclosureProps) {
	const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen)
	const isControlled = openProp !== undefined
	const isOpen = forceOpen || (isControlled ? openProp : uncontrolledOpen)

	const handleOpenChange = useCallback(
		(nextOpen: boolean) => {
			if (forceOpen) return
			if (!isControlled) setUncontrolledOpen(nextOpen)
			onOpenChange?.(nextOpen)
		},
		[forceOpen, isControlled, onOpenChange],
	)

	const contextValue = useMemo(
		() => ({ expandable: expandable && !forceOpen, isOpen }),
		[expandable, forceOpen, isOpen],
	)

	if (!expandable) {
		return (
			<TranscriptDisclosureContext.Provider value={contextValue}>
				<div className={cn("not-prose", className)}>{children}</div>
			</TranscriptDisclosureContext.Provider>
		)
	}

	return (
		<TranscriptDisclosureContext.Provider value={contextValue}>
			<Collapsible
				className={cn("not-prose", className)}
				open={isOpen}
				onOpenChange={handleOpenChange}
			>
				{children}
			</Collapsible>
		</TranscriptDisclosureContext.Provider>
	)
})

const triggerClassName =
	"flex w-full max-w-full items-center gap-1.5 -mx-1.5 rounded border-0 bg-transparent px-1.5 py-0.5 m-0 text-left text-sm text-muted-foreground/70 transition-colors hover:text-foreground"

export interface TranscriptDisclosureTriggerProps {
	label: ReactNode
	leading?: ReactNode
	trailing?: ReactNode
	className?: string
	"aria-label"?: string
}

export const TranscriptDisclosureTrigger = memo(function TranscriptDisclosureTrigger({
	label,
	leading,
	trailing,
	className,
	"aria-label": ariaLabel,
}: TranscriptDisclosureTriggerProps) {
	const { isOpen, expandable } = useTranscriptDisclosure()
	const ChevronIcon = isOpen ? ChevronDownIcon : ChevronRightIcon

	// Leading chevron keeps the disclosure affordance at a fixed position;
	// non-expandable rows render a same-width spacer so icons stay aligned.
	const chevron = expandable ? (
		<ChevronIcon aria-hidden="true" className="size-3.5 shrink-0 transition-transform" />
	) : (
		<span aria-hidden="true" className="size-3.5 shrink-0" />
	)
	const trailingSlot = trailing ? (
		<span className="ml-auto flex shrink-0 items-center">{trailing}</span>
	) : null

	if (!expandable) {
		return (
			<div className={cn(triggerClassName, className)} aria-label={ariaLabel}>
				{chevron}
				{leading}
				<span className="min-w-0 truncate">{label}</span>
				{trailingSlot}
			</div>
		)
	}

	return (
		<CollapsibleTrigger
			className={cn(triggerClassName, "hover:bg-muted/40", className)}
			aria-label={ariaLabel}
		>
			{chevron}
			{leading}
			<span className="min-w-0 truncate">{label}</span>
			{trailingSlot}
		</CollapsibleTrigger>
	)
})

export interface TranscriptDisclosureContentProps {
	children: ReactNode
	className?: string
	/** Indent content under a left guide line (aligned with the chevron) instead of a bordered box. */
	rail?: boolean
}

export const TranscriptDisclosureContent = memo(function TranscriptDisclosureContent({
	children,
	className,
	rail = false,
}: TranscriptDisclosureContentProps) {
	return (
		<CollapsibleContent
			className={cn(
				"outline-none data-closed:mt-0 data-closed:mb-0 data-closed:h-0 data-closed:overflow-hidden data-open:mt-1.5",
				rail && "ml-[7px] border-l border-border/40 pl-3",
				className,
			)}
			keepMounted={false}
		>
			{children}
		</CollapsibleContent>
	)
})
