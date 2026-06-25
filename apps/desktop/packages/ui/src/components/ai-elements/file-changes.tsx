"use client"

import { Button } from "@devo/ui/components/button"
import {
	Collapsible,
	CollapsibleContent,
	CollapsibleTrigger,
} from "@devo/ui/components/collapsible"
import { cn } from "@devo/ui/lib/utils"
import {
	CheckIcon,
	ChevronDownIcon,
	CopyIcon,
	FileIcon,
	MinusIcon,
	MoreHorizontalIcon,
	PlusIcon,
	XIcon,
} from "lucide-react"
import {
	type ComponentProps,
	createContext,
	type HTMLAttributes,
	useCallback,
	useContext,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react"
import { Diff, DiffContent, type FileContents } from "./diff"

type FileChangesStatus = "pending" | "accepted" | "rejected"

type FileChangesContextType = {
	oldFile: FileContents
	newFile: FileContents
	status: FileChangesStatus
	onStatusChange?: (status: FileChangesStatus) => void
}

const FileChangesContext = createContext<FileChangesContextType | null>(null)

const useFileChangesContext = () => {
	const context = useContext(FileChangesContext)
	if (!context) {
		throw new Error("FileChanges components must be used within a <FileChanges> component")
	}
	return context
}

export type FileChangesProps = ComponentProps<typeof Collapsible> & {
	oldFile: FileContents
	newFile: FileContents
	status?: FileChangesStatus
	onStatusChange?: (status: FileChangesStatus) => void
}

export const FileChanges = ({
	oldFile,
	newFile,
	status = "pending",
	onStatusChange,
	defaultOpen = false,
	className,
	...props
}: FileChangesProps) => (
	<FileChangesContext.Provider value={{ oldFile, newFile, status, onStatusChange }}>
		<Collapsible
			className={cn("overflow-hidden rounded-lg border bg-background shadow-sm", className)}
			defaultOpen={defaultOpen}
			{...props}
		/>
	</FileChangesContext.Provider>
)

export type FileChangesHeaderProps = ComponentProps<typeof CollapsibleTrigger>

export const FileChangesHeader = ({ className, children, ...props }: FileChangesHeaderProps) => (
	<CollapsibleTrigger
		render={
			<div
				className={cn(
					"group w-full flex cursor-pointer items-center justify-between bg-muted/50 px-4 py-3 transition-colors hover:bg-muted/70",
					className,
				)}
			/>
		}
		{...props}
	>
		{children}
	</CollapsibleTrigger>
)

export type FileChangesIconProps = HTMLAttributes<HTMLDivElement> & {
	icon?: React.ReactNode
}

export const FileChangesIcon = ({ icon, className, ...props }: FileChangesIconProps) => (
	<div className={cn("flex items-center text-muted-foreground", className)} {...props}>
		{icon ?? <FileIcon className="size-4" aria-hidden="true" />}
	</div>
)

export type FileChangesTitleProps = HTMLAttributes<HTMLParagraphElement>

export const FileChangesTitle = ({ className, children, ...props }: FileChangesTitleProps) => {
	const { newFile, oldFile } = useFileChangesContext()
	const filename = newFile.name || oldFile.name

	return (
		<p className={cn("font-medium font-mono text-foreground text-sm", className)} {...props}>
			{children ?? filename}
		</p>
	)
}

export type FileChangesStatsProps = HTMLAttributes<HTMLDivElement>

export const FileChangesStats = ({ className, ...props }: FileChangesStatsProps) => {
	const { oldFile, newFile } = useFileChangesContext()

	const stats = useMemo(() => {
		const oldLines = oldFile.content.split("\n")
		const newLines = newFile.content.split("\n")

		let additions = 0
		let deletions = 0

		const oldSet = new Set(oldLines)
		const newSet = new Set(newLines)

		for (const line of newLines) {
			if (!oldSet.has(line)) {
				additions++
			}
		}

		for (const line of oldLines) {
			if (!newSet.has(line)) {
				deletions++
			}
		}

		return { additions, deletions }
	}, [oldFile, newFile])

	return (
		<div className={cn("flex items-center gap-1.5 text-sm", className)} {...props}>
			<span className="flex items-center gap-0.5 text-diff-addition-foreground">
				<PlusIcon className="size-3" aria-hidden="true" />
				{stats.additions}
			</span>
			<span className="flex items-center gap-0.5 text-diff-deletion-foreground">
				<MinusIcon className="size-3" aria-hidden="true" />
				{stats.deletions}
			</span>
		</div>
	)
}

export type FileChangesActionsProps = HTMLAttributes<HTMLDivElement>

export const FileChangesActions = ({ className, ...props }: FileChangesActionsProps) => (
	<div
		role="group"
		className={cn("flex items-center gap-0.5", className)}
		onClick={(e) => e.stopPropagation()}
		onKeyDown={(e) => e.stopPropagation()}
		{...props}
	/>
)

export type FileChangesMoreButtonProps = ComponentProps<typeof Button>

export const FileChangesMoreButton = ({
	className,
	children,
	...props
}: FileChangesMoreButtonProps) => (
	<Button
		className={cn("size-7 text-muted-foreground hover:text-foreground", className)}
		size="icon"
		variant="ghost"
		{...props}
	>
		{children ?? <MoreHorizontalIcon className="size-4" aria-hidden="true" />}
	</Button>
)

export type FileChangesCopyButtonProps = ComponentProps<typeof Button> & {
	onCopy?: () => void
	onError?: (error: Error) => void
	timeout?: number
}

export const FileChangesCopyButton = ({
	onCopy,
	onError,
	timeout = 2000,
	className,
	children,
	...props
}: FileChangesCopyButtonProps) => {
	const [isCopied, setIsCopied] = useState(false)
	const timeoutRef = useRef<number>(0)
	const { newFile } = useFileChangesContext()

	const copyToClipboard = useCallback(async () => {
		if (typeof window === "undefined" || !navigator?.clipboard?.writeText) {
			onError?.(new Error("Clipboard API not available"))
			return
		}

		try {
			await navigator.clipboard.writeText(newFile.content)
			setIsCopied(true)
			onCopy?.()
			timeoutRef.current = window.setTimeout(() => setIsCopied(false), timeout)
		} catch (error) {
			onError?.(error as Error)
		}
	}, [newFile.content, onCopy, onError, timeout])

	useEffect(
		() => () => {
			window.clearTimeout(timeoutRef.current)
		},
		[],
	)

	const Icon = isCopied ? CheckIcon : CopyIcon

	return (
		<Button
			className={cn("size-7 text-muted-foreground hover:text-foreground", className)}
			onClick={copyToClipboard}
			size="icon"
			variant="ghost"
			{...props}
		>
			{children ?? <Icon className="size-4" aria-hidden="true" />}
		</Button>
	)
}

export type FileChangesRejectButtonProps = ComponentProps<typeof Button> & {
	onReject?: () => void
}

export const FileChangesRejectButton = ({
	onReject,
	className,
	children,
	...props
}: FileChangesRejectButtonProps) => {
	const { onStatusChange } = useFileChangesContext()

	const handleReject = useCallback(() => {
		onStatusChange?.("rejected")
		onReject?.()
	}, [onStatusChange, onReject])

	return (
		<Button
			className={cn("size-7 text-muted-foreground hover:text-foreground", className)}
			onClick={handleReject}
			size="icon"
			variant="ghost"
			{...props}
		>
			{children ?? <XIcon className="size-4" aria-hidden="true" />}
		</Button>
	)
}

export type FileChangesAcceptButtonProps = ComponentProps<typeof Button> & {
	onAccept?: () => void
}

export const FileChangesAcceptButton = ({
	onAccept,
	className,
	children,
	...props
}: FileChangesAcceptButtonProps) => {
	const { onStatusChange } = useFileChangesContext()

	const handleAccept = useCallback(() => {
		onStatusChange?.("accepted")
		onAccept?.()
	}, [onStatusChange, onAccept])

	return (
		<Button
			className={cn("size-7 text-muted-foreground hover:text-foreground", className)}
			onClick={handleAccept}
			size="icon"
			variant="ghost"
			{...props}
		>
			{children ?? <CheckIcon className="size-4" aria-hidden="true" />}
		</Button>
	)
}

export type FileChangesExpandButtonProps = HTMLAttributes<HTMLDivElement>

export const FileChangesExpandButton = ({ className, ...props }: FileChangesExpandButtonProps) => (
	<div className={cn("flex items-center text-muted-foreground", className)} {...props}>
		<ChevronDownIcon
			className="size-4 transition-transform group-data-[state=open]:rotate-180"
			aria-hidden="true"
		/>
	</div>
)

export type FileChangesContentProps = ComponentProps<typeof CollapsibleContent> & {
	showLineNumbers?: boolean
	maxHeight?: string | number
}

export const FileChangesContent = ({
	showLineNumbers = true,
	maxHeight,
	className,
	...props
}: FileChangesContentProps) => {
	const { oldFile, newFile } = useFileChangesContext()

	return (
		<CollapsibleContent
			className={cn(
				"data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=open]:animate-in",
				className,
			)}
			{...props}
		>
			<Diff mode="files" newFile={newFile} oldFile={oldFile}>
				<DiffContent maxHeight={maxHeight} showLineNumbers={showLineNumbers} />
			</Diff>
		</CollapsibleContent>
	)
}
