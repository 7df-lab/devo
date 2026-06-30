import {
	Message,
	MessageAction,
	MessageActions,
	MessageContent,
	MessageResponse,
} from "@devo/ui/components/ai-elements/message"
import {
	Reasoning,
	ReasoningContent,
	ReasoningText,
	useReasoning,
} from "@devo/ui/components/ai-elements/reasoning"
import { Shimmer } from "@devo/ui/components/ai-elements/shimmer"
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@devo/ui/components/dialog"

import {
	ArrowUpToLineIcon,
	BotIcon,
	CheckIcon,
	ChevronDownIcon,
	CopyIcon,
	FileIcon,
	GitForkIcon,
	Loader2Icon,
	Undo2Icon,
	XIcon,
} from "lucide-react"
import { memo, type ReactNode, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react"
import { useDisplayMode } from "../../hooks/use-agents"
import type { SessionCompactionStatus } from "../../atoms/compaction"
import type { ChatMessageEntry, ChatTurn as ChatTurnType } from "../../hooks/use-session-chat"
import {
	computeTurnCost,
	computeTurnWorkTime,
	formatCost,
	formatWorkDuration,
	shortModelName,
} from "../../lib/session-metrics"
import type {
	Agent,
	FilePart,
	Part,
	PermissionRequest,
	ReasoningPart,
	TextPart,
	ToolPart,
} from "../../lib/types"
import { ChatToolCall, getToolInfo, getToolSubtitle } from "./chat-tool-call"
import {
	CompactionStatusDivider,
	isCompactionStatusText,
} from "./compaction-status-divider"
import { PermissionItem } from "./chat-permission"
import { getToolCategory, type ToolCategory } from "./tool-card"

// ============================================================
// Utility functions
// ============================================================

const DEVO_ITEM_KIND_META = "devo/itemKind"
const DEVO_RESEARCH_ARTIFACT_TITLE_META = "devo/researchArtifactTitle"

/**
 * Formats a timestamp (milliseconds) to relative or absolute time.
 */
export function formatTimestamp(ms: number): string {
	const date = new Date(ms)
	return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
}

// ============================================================
// Status computation — follows into sub-agents
// ============================================================

/**
 * Computes a status string from the last active part.
 * Follows into sub-agent sessions for deeper status.
 */
function computeStatus(parts: Part[]): string {
	for (let i = parts.length - 1; i >= 0; i--) {
		const part = parts[i]
		if (part.type === "tool") {
			switch (part.tool) {
				case "task": {
					// Show what the sub-agent is actually doing
					const desc = part.state.input?.description as string | undefined
					const shortDesc = desc && desc.length > 30 ? `${desc.slice(0, 27)}...` : desc
					return shortDesc ? `Agent: ${shortDesc}` : "Delegating..."
				}
				case "todowrite":
				case "todoread":
					return "Planning..."
				case "read":
					return "Reading files..."
				case "list":
				case "grep":
				case "glob":
					return "Searching codebase..."
				case "webfetch":
					return "Fetching web content..."
				case "edit":
				case "write":
				case "apply_patch":
					return "Making edits..."
				case "bash":
					return "Running command..."
				case "question":
					return "Asking a question..."
				default:
					return `Running ${part.tool}...`
			}
		}
		if (part.type === "reasoning") return "Thinking..."
		if (part.type === "text") return "Composing response..."
	}
	return "Working..."
}

// ============================================================
// Synthetic message helpers
// ============================================================

function isSyntheticMessage(entry: ChatMessageEntry): boolean {
	const textParts = entry.parts.filter((p): p is TextPart => p.type === "text")
	// All text parts are synthetic (e.g. compaction continuation, shell execution)
	if (textParts.length > 0 && textParts.every((p) => p.synthetic === true)) return true
	// No text parts at all — e.g. a user message with only a compaction part
	if (textParts.length === 0 && entry.parts.length > 0) return true
	return false
}

function getUserText(entry: ChatMessageEntry): string {
	return entry.parts
		.filter((p): p is TextPart => p.type === "text" && !p.synthetic)
		.map((p) => p.text)
		.join("\n")
}

function getSyntheticLabel(entry: ChatMessageEntry): string {
	const text = entry.parts
		.filter((p): p is TextPart => p.type === "text")
		.map((p) => p.text)
		.join("\n")
		.toLowerCase()

	if (text.includes("continue if you have next steps")) return "Auto-continued after compaction"
	if (text.includes("summarize the task tool output")) return "Auto-continued after task"
	if (text.includes("tool was executed by the user")) return "Shell command executed"
	if (text.includes("plan has been approved")) return "Plan approved"
	if (text.includes("enter plan mode")) return "Entered plan mode"
	if (text.includes("switch") && text.includes("plan")) return "Mode switched"
	// No text parts — check for compaction part (user message that triggers compaction)
	if (entry.parts.some((p) => p.type === "compaction")) return "Compacting conversation"
	return "Auto-continued"
}

function getFileParts(entry: ChatMessageEntry): FilePart[] {
	return entry.parts.filter(
		(p): p is FilePart =>
			p.type === "file" && (p.mime.startsWith("image/") || p.mime === "application/pdf"),
	)
}

// ============================================================
// Attachment grid
// ============================================================

const AttachmentGrid = memo(function AttachmentGrid({
	files,
	onDelete,
}: { files: FilePart[]; onDelete?: (file: FilePart) => void }) {
	if (files.length === 0) return null
	return (
		<div className="flex flex-wrap gap-2">
			{files.map((file) => (
				<AttachmentThumbnail key={file.id} file={file} onDelete={onDelete} />
			))}
		</div>
	)
})

function AttachmentThumbnail({
	file,
	onDelete,
}: { file: FilePart; onDelete?: (file: FilePart) => void }) {
	const isImage = file.mime.startsWith("image/")
	const [deleting, setDeleting] = useState(false)

	const handleDelete = useCallback(
		async (e: React.MouseEvent) => {
			e.stopPropagation()
			if (!onDelete || deleting) return
			setDeleting(true)
			try {
				await onDelete(file)
			} finally {
				setDeleting(false)
			}
		},
		[onDelete, file, deleting],
	)

	return (
		<Dialog>
			<div className="group/thumb relative size-16 shrink-0">
				{onDelete && (
					<button
						type="button"
						onClick={handleDelete}
						disabled={deleting}
						className="absolute -right-1 -top-1 z-10 flex size-4 items-center justify-center rounded-full bg-destructive text-destructive-foreground opacity-0 shadow-sm transition-opacity hover:bg-destructive/90 group-hover/thumb:opacity-100 disabled:opacity-50"
						title="Remove attachment"
					>
						<XIcon className="size-2.5" />
					</button>
				)}
				<DialogTrigger
					render={
						<button
							type="button"
							className="size-full overflow-hidden rounded-lg border border-border bg-muted transition-colors hover:border-muted-foreground/30"
						/>
					}
				>
					{isImage ? (
						<img
							src={file.url}
							alt={file.filename ?? "Image attachment"}
							className="size-full object-cover"
						/>
					) : (
						<div className="flex size-full items-center justify-center">
							<FileIcon className="size-6 text-muted-foreground" />
						</div>
					)}
					{file.filename && (
						<div className="absolute inset-x-0 bottom-0 bg-black/60 px-1 py-0.5 text-[9px] leading-tight text-white opacity-0 transition-opacity group-hover/thumb:opacity-100">
							<span className="line-clamp-1">{file.filename}</span>
						</div>
					)}
				</DialogTrigger>
			</div>
			<DialogContent className="max-h-[90vh] max-w-4xl overflow-auto p-0">
				<DialogTitle className="sr-only">{file.filename ?? "Attachment preview"}</DialogTitle>
				{isImage ? (
					<img
						src={file.url}
						alt={file.filename ?? "Image attachment"}
						className="max-h-[85vh] w-full object-contain"
					/>
				) : (
					<div className="flex flex-col items-center justify-center gap-2 p-8">
						<FileIcon className="size-12 text-muted-foreground" />
						<p className="text-sm text-muted-foreground">{file.filename ?? "PDF attachment"}</p>
					</div>
				)}
			</DialogContent>
		</Dialog>
	)
}

// ============================================================
// Part extraction helpers
// ============================================================

/** A renderable part — either a tool call, an intermediate text block, or reasoning */
type RenderablePart =
	| { kind: "tool"; part: ToolPart }
	| { kind: "text"; id: string; text: string; metadata?: Record<string, unknown> }
	| { kind: "reasoning"; part: ReasoningPart }

type TextRenderablePart = Extract<RenderablePart, { kind: "text" }>

/**
 * Flattens all assistant parts into an ordered list of renderable items
 * AND extracts the tool-only subset in a single pass.
 * Preserves the natural order: text, reasoning, tool, text, tool, text...
 * Filters out synthetic text, todoread without output, and empty text.
 * Strips OpenRouter [REDACTED] chunks from reasoning and skips empty reasoning.
 */
function getPartsAndTools(assistantMessages: ChatMessageEntry[]): {
	ordered: RenderablePart[]
	tools: ToolPart[]
} {
	const ordered: RenderablePart[] = []
	const tools: ToolPart[] = []
	for (const msg of assistantMessages) {
		for (const part of msg.parts) {
			if (part.type === "tool") {
				tools.push(part)
				if (part.tool === "todoread" && part.state.status !== "completed") continue
				ordered.push({ kind: "tool", part })
			} else if (part.type === "text" && !part.synthetic && part.text.trim()) {
				if (isCompactionStatusText(part.text)) continue
				const metadata = (part as { metadata?: Record<string, unknown> }).metadata
				ordered.push({ kind: "text", id: part.id, text: part.text, metadata })
			} else if (part.type === "reasoning") {
				// Strip OpenRouter's encrypted [REDACTED] chunks
				const cleaned = part.text.replace("[REDACTED]", "").trim()
				if (cleaned) {
					ordered.push({ kind: "reasoning", part })
				}
			}
		}
	}
	return { ordered, tools }
}

function hasCompactionStatusMarker(assistantMessages: ChatMessageEntry[]): boolean {
	return assistantMessages.some((msg) =>
		msg.parts.some((part) => part.type === "text" && isCompactionStatusText(part.text)),
	)
}

/**
 * Gets the last text part's content — used for the final streaming response
 * and the copy action. Returns undefined if no text parts exist.
 */
function getLastResponseText(orderedParts: RenderablePart[]): string | undefined {
	for (let i = orderedParts.length - 1; i >= 0; i--) {
		const item = orderedParts[i]
		if (item.kind === "text") return item.text
	}
	return undefined
}

function splitCompletedTurnParts(orderedParts: RenderablePart[]): {
	completedProcessParts: RenderablePart[]
	finalResponsePart: TextRenderablePart | undefined
} {
	let finalResponseIndex = -1
	for (let i = orderedParts.length - 1; i >= 0; i--) {
		if (orderedParts[i].kind === "text") {
			finalResponseIndex = i
			break
		}
	}

	if (finalResponseIndex === -1) {
		return { completedProcessParts: orderedParts, finalResponsePart: undefined }
	}

	const finalResponsePart = orderedParts[finalResponseIndex] as TextRenderablePart
	const completedProcessParts = orderedParts.filter((_, index) => index !== finalResponseIndex)
	return { completedProcessParts, finalResponsePart }
}

function researchArtifactTitle(item: TextRenderablePart): string | undefined {
	const metadata = item.metadata
	if (metadata?.[DEVO_ITEM_KIND_META] !== "research_artifact") return undefined
	const title = metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META]
	return typeof title === "string" && title.trim() ? title : undefined
}

function ResearchArtifactBlock({ item }: { item: TextRenderablePart }) {
	const title = researchArtifactTitle(item)
	if (!title) {
		return (
			<Message from="assistant">
				<MessageContent>
					<MessageResponse>{item.text}</MessageResponse>
				</MessageContent>
			</Message>
		)
	}
	return (
		<div className="border-l border-primary/30 pl-3">
			<div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
				<FileIcon className="size-3" aria-hidden="true" />
				<span>{title}</span>
			</div>
			<Message from="assistant">
				<MessageContent>
					<MessageResponse>{item.text}</MessageResponse>
				</MessageContent>
			</Message>
		</div>
	)
}

function getError(assistantMessages: ChatMessageEntry[]): string | undefined {
	for (const msg of assistantMessages) {
		if (msg.info.role === "assistant" && msg.info.error) {
			const error = msg.info.error
			const errorData = error.data
			// Most error types have a `message` string in data
			if ("message" in errorData && errorData.message) {
				return typeof errorData.message === "string" ? errorData.message : String(errorData.message)
			}
			// Fallback: use the error name (e.g. "MessageOutputLengthError") +
			// any stringifiable data for types like MessageOutputLengthError
			// whose data is { [key: string]: unknown }
			const dataStr = Object.keys(errorData).length > 0 ? JSON.stringify(errorData) : undefined
			return dataStr ? `${error.name}: ${dataStr}` : error.name
		}
	}
	return undefined
}

function LazyReasoningContent({
	children,
	animated,
}: {
	children: ReactNode
	animated?: boolean
}) {
	const { isOpen, isStreaming } = useReasoning()
	if (!isOpen && !isStreaming) return null
	return <ReasoningContent animated={animated}>{children}</ReasoningContent>
}

function TranscriptReasoningLiveCue() {
	return (
		<div
			aria-label="Reasoning details"
			className="border-l border-border/70 pl-3 text-sm text-muted-foreground/70"
		>
			<Shimmer duration={1}>Thinking...</Shimmer>
		</div>
	)
}

function TranscriptReasoningBlock({ text, animated }: { text: string; animated?: boolean }) {
	return (
		<div
			aria-label="Reasoning details"
			className="border-l border-border/70 pl-3 text-sm text-muted-foreground/80"
		>
			<ReasoningText animated={animated}>{text}</ReasoningText>
		</div>
	)
}

// ============================================================
// Turn comparison for memo
// ============================================================

/**
 * Lightweight fingerprint for a ChatMessageEntry to detect real content changes
 * without comparing the full object tree. Mirrors the logic in session-chat.ts
 * but kept local to avoid coupling.
 */
function messageEntryFingerprint(entry: ChatMessageEntry): string {
	const lastPart = entry.parts.at(-1)
	const completed = entry.info.role === "assistant" ? (entry.info.time.completed ?? 0) : 0
	let textLen = 0
	const toolSegments: string[] = []
	const textMetadataSegments: string[] = []
	for (const part of entry.parts) {
		if (part.type === "text" || part.type === "reasoning") {
			textLen += part.text.length
			if (part.type === "text") {
				const metadata = (part as { metadata?: Record<string, unknown> }).metadata
				if (metadata?.[DEVO_ITEM_KIND_META] === "research_artifact") {
					textMetadataSegments.push(
						`${part.id}:${metadata[DEVO_ITEM_KIND_META]}:${metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META] ?? ""}`,
					)
				}
			}
		} else if (part.type === "tool") {
			const outLen =
				part.state.status === "completed"
					? part.state.output.length
					: part.state.status === "error"
						? part.state.error.length
						: 0
			toolSegments.push(`${part.id}:${part.state.status}:${outLen}`)
		}
	}
	return `${entry.info.id}:${completed}:${entry.parts.length}:${lastPart?.id ?? ""}:${textLen}:${textMetadataSegments.join(",")}:${toolSegments.join(",")}`
}

/** Compare two turns by content fingerprint rather than reference equality */
function areTurnsEqual(a: ChatTurnType, b: ChatTurnType): boolean {
	if (a === b) return true
	if (a.id !== b.id) return false
	if (messageEntryFingerprint(a.userMessage) !== messageEntryFingerprint(b.userMessage))
		return false
	if (a.assistantMessages.length !== b.assistantMessages.length) return false
	for (let i = 0; i < a.assistantMessages.length; i++) {
		if (
			messageEntryFingerprint(a.assistantMessages[i]) !==
			messageEntryFingerprint(b.assistantMessages[i])
		)
			return false
	}
	return true
}

// ============================================================
// Default mode helpers — tool grouping
// ============================================================

/**
 * Groups consecutive tool parts of the same category into summary items.
 * Interleaves text and reasoning between groups to preserve natural order.
 *
 * Example output:
 *   text: "Let me look at the code..."
 *   tool-group: { category: "explore", tools: [read, grep, glob] }
 *   text: "I found the issue, let me fix it..."
 *   tool-group: { category: "edit", tools: [edit, write] }
 *   tool-group: { category: "run", tools: [bash] }
 */
type StreamItem =
	| { kind: "text"; id: string; text: string; metadata?: Record<string, unknown> }
	| { kind: "reasoning-process"; items: (RenderablePart & { kind: "reasoning" | "tool" })[] }
	| { kind: "tool-group"; category: ToolCategory; tools: ToolPart[] }

function groupPartsForStream(ordered: RenderablePart[]): StreamItem[] {
	const items: StreamItem[] = []
	let currentGroup: { category: ToolCategory; tools: ToolPart[] } | null = null
	let currentProcessGroup: (RenderablePart & { kind: "reasoning" | "tool" })[] = []

	const hasReasoning = ordered.some((p) => p.kind === "reasoning")

	const flushGroup = () => {
		if (currentGroup) {
			items.push({ kind: "tool-group", ...currentGroup })
			currentGroup = null
		}
	}

	const flushProcessGroup = () => {
		if (currentProcessGroup.length > 0) {
			items.push({ kind: "reasoning-process", items: currentProcessGroup })
			currentProcessGroup = []
		}
	}

	for (const part of ordered) {
		if (hasReasoning && (part.kind === "reasoning" || part.kind === "tool")) {
			currentProcessGroup.push(part)
		} else if (!hasReasoning && part.kind === "tool") {
			const category = getToolCategory(part.part.tool)
			if (currentGroup && currentGroup.category === category) {
				currentGroup.tools.push(part.part)
			} else {
				flushGroup()
				currentGroup = { category, tools: [part.part] }
			}
		} else {
			flushGroup()
			flushProcessGroup()
			if (part.kind === "text") {
				items.push({ kind: "text", id: part.id, text: part.text, metadata: part.metadata })
			}
		}
	}
	flushGroup()
	flushProcessGroup()
	return items
}

/**
 * Generates a human-readable summary for a group of tools in the same category.
 * Returns text like "Read 3 files", "Edited foo.tsx, bar.tsx", "Ran 2 commands".
 */
function describeToolGroup(
	category: ToolCategory,
	tools: ToolPart[],
	projectRoot?: string | null,
): string {
	const count = tools.length

	// For small groups, list specific targets
	if (count <= 3) {
		const details = tools
			.map((t) => getToolSubtitle(t, { projectRoot }))
			.filter(Boolean)
			.map((s) => {
				// Shorten file paths to just the filename
				const parts = s!.split("/")
				return parts.length > 1 ? parts[parts.length - 1] : s
			})

		if (details.length > 0) {
			switch (category) {
				case "explore":
					return count === 1 ? `Read ${details[0]}` : `Read ${details.join(", ")}`
				case "edit":
					return count === 1 ? `Edited ${details[0]}` : `Edited ${details.join(", ")}`
				case "run":
					return count === 1
						? `Ran ${details[0]}`
						: `Ran ${count} commands`
				case "delegate":
					return count === 1 ? `Delegated: ${details[0]}` : `Delegated ${count} tasks`
				case "fetch":
					return count === 1 ? `Fetched ${details[0]}` : `Fetched ${count} URLs`
				case "ask":
					return "Asked a question"
				case "plan":
					return "Updated plan"
				default:
					return `Ran ${details.join(", ")}`
			}
		}
	}

	// For larger groups, use count-based summaries
	switch (category) {
		case "explore":
			return `Explored ${count} files`
		case "edit":
			return `Edited ${count} files`
		case "run":
			return `Ran ${count} commands`
		case "delegate":
			return `Delegated ${count} tasks`
		case "fetch":
			return `Fetched ${count} URLs`
		case "ask":
			return `Asked ${count} questions`
		case "plan":
			return "Updated plan"
		default:
			return `Ran ${count} tools`
	}
}

/**
 * Returns true if any tool in the group is still running/pending.
 */
function isGroupRunning(tools: ToolPart[]): boolean {
	return tools.some((t) => t.state.status === "running" || t.state.status === "pending")
}

/**
 * Returns true if any tool in the group has an error.
 */
function isGroupError(tools: ToolPart[]): boolean {
	return tools.some((t) => t.state.status === "error")
}

/** Renders a single tool group summary as a collapsible disclosure row */
const ToolGroupSummary = memo(function ToolGroupSummary({
	category,
	tools,
	isActiveTurn,
	projectRoot,
}: {
	category: ToolCategory
	tools: ToolPart[]
	isActiveTurn: boolean
	projectRoot?: string | null
}) {
	const [expanded, setExpanded] = useState(false)
	const description = describeToolGroup(category, tools, projectRoot)
	const running = isGroupRunning(tools)
	const hasError = isGroupError(tools)
	const { icon: GroupIcon } = getToolInfo(tools[0].tool)

	return (
		<div className="space-y-2">
			<button
				type="button"
				onClick={() => setExpanded((v) => !v)}
				className="flex w-full items-center gap-2 rounded-md bg-muted/20 px-3 py-1.5 text-[12px] transition-colors hover:bg-muted/40"
			>
				<GroupIcon
					className={`size-3.5 shrink-0 ${
						hasError
							? "text-red-400"
							: running
								? "animate-pulse text-muted-foreground"
								: "text-muted-foreground/50"
					}`}
				/>
				<span className={`flex-1 text-left ${hasError ? "text-red-400" : "text-muted-foreground/70"}`}>
					{description}
				</span>
				{running && !expanded && (
					<Loader2Icon className="size-3 animate-spin text-muted-foreground/30" />
				)}
				<ChevronDownIcon
					className={`size-3 shrink-0 text-muted-foreground/30 transition-transform ${expanded ? "" : "-rotate-90"}`}
				/>
			</button>
			{expanded && (
				<div className="space-y-2 pl-3">
					{tools.map((tool) => (
						<ChatToolCall
							key={tool.id}
							part={tool}
							isActiveTurn={isActiveTurn}
							projectRoot={projectRoot}
						/>
					))}
				</div>
			)}
		</div>
	)
})

// ============================================================
// ChatTurnComponent
// ============================================================

type PendingPermission = {
	request: PermissionRequest
	sessionId: string
}

interface ChatTurnProps {
	turn: ChatTurnType
	isLast: boolean
	isWorking: boolean
	agent?: Agent
	pendingPermission?: PendingPermission
	isConnected?: boolean
	compactionStatus?: SessionCompactionStatus | null
	onApprovePermission?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: "once" | "always",
	) => Promise<void>
	onDenyPermission?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
	) => Promise<void>
	/** Revert to this turn's user message (for per-turn undo) */
	onRevertToMessage?: (messageId: string) => Promise<void>
	/** Fork the conversation from this turn boundary */
	onForkFromTurn?: () => Promise<void>
	/** Delete a specific part from a message (for error recovery) */
	onDeletePart?: (sessionId: string, messageId: string, partId: string) => Promise<void>
	/** Controlled verbose step expansion state, used by virtualized transcripts. */
	stepsExpanded?: boolean
	onStepsExpandedChange?: (turnId: string, expanded: boolean) => void
}

function pendingPermissionFingerprint(permission: PendingPermission | undefined): string {
	if (!permission) return ""
	const requestId =
		typeof permission.request.id === "string"
			? permission.request.id
			: typeof permission.request.requestID === "string"
				? permission.request.requestID
				: ""
	return `${permission.sessionId}:${requestId}`
}

function WorkingTurnStatusStrip({ turn }: { turn: ChatTurnType }) {
	const [display, setDisplay] = useState(() =>
		formatWorkDuration(computeTurnWorkTime(turn, { active: true })),
	)

	useEffect(() => {
		const updateDisplay = () => {
			setDisplay(formatWorkDuration(computeTurnWorkTime(turn, { active: true })))
		}
		updateDisplay()
		const id = setInterval(updateDisplay, 1_000)
		return () => clearInterval(id)
	}, [turn])

	return (
		<div className="space-y-2 pt-1">
			<div className="text-sm tabular-nums text-muted-foreground/70">
				{"Working for "}
				{display}
			</div>
			<div className="h-px bg-border/70" />
		</div>
	)
}

function CompletedTurnProcessDisclosure({
	duration,
	expanded,
	hasProcessDetails,
	onToggle,
}: {
	duration: string
	expanded: boolean
	hasProcessDetails: boolean
	onToggle: () => void
}) {
	if (!duration && !hasProcessDetails) return null

	const content = (
		<>
			<span>
				{duration ? "Worked for " : "Worked"}
				{duration}
			</span>
			{hasProcessDetails && (
				<ChevronDownIcon
					className={expanded ? "size-4 rotate-180 transition-transform" : "size-4 transition-transform"}
					aria-hidden="true"
				/>
			)}
		</>
	)

	if (!hasProcessDetails) {
		return (
			<div className="flex w-full items-center gap-1.5 border-b border-border/70 pb-2 text-sm tabular-nums text-muted-foreground/70">
				{content}
			</div>
		)
	}

	return (
		<button
			type="button"
			onClick={onToggle}
			aria-expanded={expanded}
			className="flex w-full items-center gap-1.5 border-b border-border/70 pb-2 text-left text-sm tabular-nums text-muted-foreground/70 transition-colors hover:text-foreground"
		>
			{content}
		</button>
	)
}

/**
 * Renders a single turn: user message + assistant response.
 *
 * Two modes based on turn state:
 * - **Active turn** (last + working): tool calls are individually rendered with
 *   per-tool ToolCards, smart default expand/collapse, and live activity.
 * - **Completed turn**: icon-pill summary bar with one-click expand to show
 *   individual tools. Response text is always visible.
 *
 * Display mode preference (default/verbose) modifies behavior:
 * - default: interleaved text + grouped tool summaries as inline chips.
 *   Tool groups batch consecutive same-category calls (e.g., "Explored 3 files",
 *   "Edited foo.tsx, bar.tsx"). A "Show N steps" toggle reveals full tool cards.
 * - verbose: all turns show all tools expanded with full content (tool cards)
 */
export const ChatTurnComponent = memo(
	function ChatTurnComponent({
		turn,
		isLast,
		isWorking,
		agent,
		pendingPermission,
		isConnected = false,
		compactionStatus,
		onApprovePermission,
		onDenyPermission,
		onRevertToMessage,
		onForkFromTurn,
		onDeletePart,
		stepsExpanded: controlledStepsExpanded,
		onStepsExpandedChange,
	}: ChatTurnProps) {
		const [uncontrolledStepsExpanded, setUncontrolledStepsExpanded] = useState(false)
		const [completedProcessExpanded, setCompletedProcessExpanded] = useState(false)
		const [copied, setCopied] = useState(false)
		const displayMode = useDisplayMode()
		const toolPathRoot = agent?.worktreePath ?? agent?.directory ?? agent?.projectDirectory
		const turnRef = useRef<HTMLDivElement>(null)
		const stepsExpanded = controlledStepsExpanded ?? uncontrolledStepsExpanded
		useEffect(() => {
			setCompletedProcessExpanded(false)
		}, [turn.id])

		const setStepsExpanded = useCallback(
			(expanded: boolean) => {
				if (onStepsExpandedChange) {
					onStepsExpandedChange(turn.id, expanded)
				} else {
					setUncontrolledStepsExpanded(expanded)
				}
			},
			[onStepsExpandedChange, turn.id],
		)

		const isSynthetic = useMemo(() => isSyntheticMessage(turn.userMessage), [turn.userMessage])
		const userText = useMemo(() => getUserText(turn.userMessage), [turn.userMessage])
		const syntheticLabel = useMemo(
			() => (isSynthetic ? getSyntheticLabel(turn.userMessage) : ""),
			[isSynthetic, turn.userMessage],
		)
		const userFiles = useMemo(() => getFileParts(turn.userMessage), [turn.userMessage])

		// Ordered parts + tool-only subset in a single pass (avoids double iteration)
		const { ordered: orderedParts } = useMemo(
			() => getPartsAndTools(turn.assistantMessages),
			[turn.assistantMessages],
		)

		const { completedProcessParts, finalResponsePart } = useMemo(
			() => splitCompletedTurnParts(orderedParts),
			[orderedParts],
		)
		const hasCompactionMarker = useMemo(
			() => hasCompactionStatusMarker(turn.assistantMessages),
			[turn.assistantMessages],
		)
		const displayedCompactionStatus: SessionCompactionStatus | null = hasCompactionMarker
			? compactionStatus === "completed"
				? "completed"
				: "started"
			: null

		// The last text for streaming display and copy action
		const rawResponseText = useMemo(() => getLastResponseText(orderedParts), [orderedParts])
		const responseText = useDeferredValue(rawResponseText)

		const errorText = useMemo(() => getError(turn.assistantMessages), [turn.assistantMessages])

		// Compute status by walking the last message's parts in reverse — no
		// need to flatMap all messages into a temporary array.
		const statusText = useMemo(() => {
			for (let m = turn.assistantMessages.length - 1; m >= 0; m--) {
				const status = computeStatus(turn.assistantMessages[m].parts)
				if (status !== "Working...") return status
			}
			return "Working..."
		}, [turn.assistantMessages])

		const working = isLast && isWorking
		// User requirement: queue state belongs in the composer status stack;
		// this transcript must not infer queued state from an empty assistant response.
		const processOrderedParts = working ? orderedParts : completedProcessParts
		const processToolParts = useMemo(
			() => processOrderedParts.flatMap((part) => (part.kind === "tool" ? [part.part] : [])),
			[processOrderedParts],
		)
		const hasSteps = processToolParts.length > 0
		const hasCompletedProcessDetails = !working && completedProcessParts.length > 0
		const processSectionVisible = working || (hasCompletedProcessDetails && completedProcessExpanded)

		const duration = useMemo(() => {
			const workTimeMs = computeTurnWorkTime(turn, { active: working })
			return workTimeMs >= 1000 ? formatWorkDuration(workTimeMs) : ""
		}, [turn, working])
		const turnCostStr = useMemo(() => {
			const cost = computeTurnCost(turn)
			return cost > 0 ? formatCost(cost) : ""
		}, [turn])
		const turnModel = useMemo(() => {
			for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
				const info = turn.assistantMessages[i].info
				if (info.role === "assistant" && info.modelID) {
					return shortModelName(info.modelID)
				}
			}
			return ""
		}, [turn.assistantMessages])

		// Determine if tools should be shown individually (active turn behavior)
		const isActiveTurn = working
		const isVerbose = displayMode === "verbose"

		// In default mode, we render a "stream" of grouped tool summaries + text.
		// In verbose mode, we render full tool cards.
		// stepsExpanded forces verbose rendering on a per-turn basis.
		const showVerboseTools = isVerbose || stepsExpanded

		const verboseOrderedParts = useMemo(() => {
			if (!stepsExpanded || isVerbose || working) return processOrderedParts
			const stepParts = processOrderedParts.filter((part) => part.kind !== "text")
			const textParts = processOrderedParts.filter((part) => part.kind === "text")
			return [...stepParts, ...textParts]
		}, [isVerbose, processOrderedParts, stepsExpanded, working])

		// Grouped stream items for the default (non-verbose) rendering path
		const defaultOrderedParts = processOrderedParts

		const streamItems = useMemo(
			() => (showVerboseTools ? [] : groupPartsForStream(defaultOrderedParts)),
			[showVerboseTools, defaultOrderedParts],
		)

		// In default mode, process text is rendered inline within the stream.
		// In verbose mode, process text is inline within the expanded ordered parts.
		const textAlreadyInline =
			processSectionVisible && processOrderedParts.some((p) => p.kind === "text")

		const handleCopyResponse = useCallback(async () => {
			if (!responseText) return
			await navigator.clipboard.writeText(responseText)
			setCopied(true)
			setTimeout(() => setCopied(false), 2000)
		}, [responseText])

		const handleRevertHere = useCallback(async () => {
			if (!onRevertToMessage) return
			await onRevertToMessage(turn.userMessage.info.id)
		}, [onRevertToMessage, turn.userMessage.info.id])

		const handleScrollToTop = useCallback(() => {
			turnRef.current?.scrollIntoView({ behavior: "smooth", block: "start" })
		}, [])

		const handleToggleCompletedProcess = useCallback(() => {
			setCompletedProcessExpanded((expanded) => !expanded)
		}, [])

		const [forking, setForking] = useState(false)
		const handleFork = useCallback(async () => {
			if (!onForkFromTurn || forking) return
			setForking(true)
			try {
				await onForkFromTurn()
			} finally {
				setForking(false)
			}
		}, [onForkFromTurn, forking])

		const handleDeleteFile = useCallback(
			async (file: FilePart) => {
				if (!onDeletePart) return
				await onDeletePart(file.sessionID, file.messageID, file.id)
			},
			[onDeletePart],
		)

		const handleDeleteToolPart = useCallback(
			async (toolPart: ToolPart) => {
				if (!onDeletePart) return
				await onDeletePart(toolPart.sessionID, toolPart.messageID, toolPart.id)
			},
			[onDeletePart],
		)

		const stepsToggle =
			!isVerbose && !isActiveTurn && hasSteps ? (
				<button
					type="button"
					onClick={() => setStepsExpanded(!stepsExpanded)}
					className="flex items-center gap-1.5 text-[11px] text-muted-foreground/40 transition-colors hover:text-foreground"
				>
					<ChevronDownIcon className={showVerboseTools ? "size-3" : "size-3 -rotate-90"} />
					<span>
						{showVerboseTools ? "Hide" : "Show"} {processToolParts.length}{" "}
						{processToolParts.length === 1 ? "step" : "steps"}
					</span>
					<span>
						{turnModel && `· ${turnModel} `}
						{duration && `· ${duration} `}
						{turnCostStr && `· ${turnCostStr}`}
					</span>
				</button>
			) : null

		return (
			<div ref={turnRef} className="group/turn space-y-4">
				{/* User message */}
				{isSynthetic ? (
					<div className="flex items-center justify-end gap-1.5 text-[11px] italic text-muted-foreground/50">
						<BotIcon className="size-3" aria-hidden="true" />
						<span>{syntheticLabel}</span>
					</div>
				) : (
					<Message from="user">
						<MessageContent>
							{userFiles.length > 0 && (
								<AttachmentGrid
									files={userFiles}
									onDelete={onDeletePart ? handleDeleteFile : undefined}
								/>
							)}
							<p className="whitespace-pre-wrap">{userText}</p>
						</MessageContent>
					</Message>
				)}

				{working && <WorkingTurnStatusStrip turn={turn} />}
				{!working && (duration || hasCompletedProcessDetails) && (
					<CompletedTurnProcessDisclosure
						duration={duration}
						expanded={completedProcessExpanded}
						hasProcessDetails={hasCompletedProcessDetails}
						onToggle={handleToggleCompletedProcess}
					/>
				)}

				{/* Tool calls + reasoning section */}
				{processSectionVisible && (
					<div className="space-y-2">
						{!working && completedProcessExpanded && stepsToggle}

						{/* ── Default mode: interleaved text + grouped tool summaries ── */}
						{!showVerboseTools && (
							<div className="space-y-3">
								{streamItems.map((item, idx) => {
									if (item.kind === "text") {
										return (
											<div key={item.id} className="py-0.5">
												<ResearchArtifactBlock item={item} />
											</div>
										)
									}
									if (item.kind === "reasoning-process") {
										const firstReasoning = item.items.find((p) => p.kind === "reasoning") as { kind: "reasoning", part: ReasoningPart } | undefined
										const lastItem = item.items[item.items.length - 1]

										const durationSec = firstReasoning?.part.time.end
											? Math.ceil((firstReasoning.part.time.end - firstReasoning.part.time.start) / 1000)
											: undefined
										const isStreaming = working && (!lastItem || (lastItem.kind === "reasoning" && !lastItem.part.time.end))

										return (
											<Reasoning
												key={`process-${idx}`}
												isStreaming={isStreaming}
												duration={durationSec}
												defaultOpen={isStreaming ? undefined : completedProcessExpanded}
											>
												{isStreaming && <TranscriptReasoningLiveCue />}
												<LazyReasoningContent animated={isStreaming}>
													<div className="flex flex-col gap-4">
														{item.items.map((processItem, i) => {
															if (processItem.kind === "reasoning") {
																const text = processItem.part.text.replace("[REDACTED]", "").trim()
																if (!text) return null
																return (
																	<TranscriptReasoningBlock key={processItem.part.id} text={text} animated={isStreaming && i === item.items.length - 1} />
																)
															}
															if (processItem.kind === "tool") {
																return (
																	<div key={processItem.part.id} className="pl-3">
																		<ChatToolCall
																			part={processItem.part}
																			isActiveTurn={isActiveTurn}
																			projectRoot={toolPathRoot}
																		/>
																	</div>
																)
															}
															return null
														})}
													</div>
												</LazyReasoningContent>
											</Reasoning>
										)
									}
									if (item.kind === "tool-group") {
										if (item.tools.length === 1) {
											return (
												<ChatToolCall
													key={item.tools[0].id}
													part={item.tools[0]}
													isActiveTurn={isActiveTurn}
													projectRoot={toolPathRoot}
												/>
											)
										}
										return (
											<ToolGroupSummary
												key={`group-${idx}-${item.tools[0].id}`}
												category={item.category}
												tools={item.tools}
												isActiveTurn={isActiveTurn}
												projectRoot={toolPathRoot}
											/>
										)
									}
									return null
								})}
								{/* Live status while the agent is still working */}
								{working && hasSteps && (
									<div className="flex items-center gap-2 px-1 text-xs text-muted-foreground">
										<Loader2Icon className="size-3 animate-spin text-muted-foreground/30" />
										<Shimmer className="text-[11px]">{statusText}</Shimmer>
									</div>
								)}
							</div>
						)}


						{/* ── Verbose mode: full tool cards ──────────────────────── */}

						{showVerboseTools && (
							<div className="space-y-3.5">
								{verboseOrderedParts.map((item) => {
									if (item.kind === "tool") {
										return (
											<ChatToolCall
												key={item.part.id}
												part={item.part}
												isActiveTurn={isActiveTurn}
												turnHasError={!!errorText}
												onDelete={onDeletePart ? handleDeleteToolPart : undefined}
												projectRoot={toolPathRoot}
											/>
										)
									}
									if (item.kind === "reasoning") {
										const reasoningText = item.part.text.replace("[REDACTED]", "").trim()
										if (!reasoningText) return null
										const durationSec = item.part.time.end
											? Math.ceil((item.part.time.end - item.part.time.start) / 1000)
											: undefined
										const isReasoningStreaming = !item.part.time.end && working
										return (
											<Reasoning
												key={item.part.id}
												isStreaming={isReasoningStreaming}
												duration={durationSec}
												defaultOpen={isReasoningStreaming ? undefined : completedProcessExpanded}
											>
												{isReasoningStreaming && <TranscriptReasoningLiveCue />}
												<LazyReasoningContent animated={isReasoningStreaming}>
													<TranscriptReasoningBlock text={reasoningText} animated={isReasoningStreaming} />
												</LazyReasoningContent>
											</Reasoning>
										)
									}
									return (
										<div key={item.id} className="py-0.5">
											<ResearchArtifactBlock item={item} />
										</div>
									)
								})}
							</div>
						)}
					</div>
				)}

				{pendingPermission && agent && (
					<PermissionItem
						agent={agent}
						permission={pendingPermission.request}
						onApprove={onApprovePermission}
						onDeny={onDenyPermission}
						isConnected={isConnected}
						isFromSubAgent={pendingPermission.sessionId !== agent.sessionId}
					/>
				)}

				{/* Error */}
				{errorText && (
					<div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
						{errorText.length > 300 ? `${errorText.slice(0, 300)}...` : errorText}
					</div>
				)}

				{/* Completed final response */}
				{!working && finalResponsePart && responseText && (
					researchArtifactTitle(finalResponsePart) ? (
						<ResearchArtifactBlock item={{ ...finalResponsePart, text: responseText }} />
					) : (
						<Message from="assistant">
							<MessageContent>
								<MessageResponse>{responseText}</MessageResponse>
							</MessageContent>
						</Message>
					)
				)}

				{/* Streaming response — visible while working, when text isn't already inline */}
				{working && responseText && !textAlreadyInline && (
					<Message from="assistant">
						<MessageContent>
							<MessageResponse animated>{responseText}</MessageResponse>
						</MessageContent>
					</Message>
				)}

				{/* User requirement: render compaction lifecycle as a transcript divider,
				   not as a normal assistant message that can hide the previous reply. */}
				{displayedCompactionStatus && (
					<CompactionStatusDivider status={displayedCompactionStatus} />
				)}

				{/* Per-turn metadata — shown on completed turns so badges are visible after long responses */}
				{!working && turn.assistantMessages.length > 0 && (turnModel || turnCostStr) && (
					<div className="flex items-center gap-1.5 text-[11px] tabular-nums text-muted-foreground/40">
						{turnModel && <span>{turnModel}</span>}
						{turnModel && turnCostStr && <span>·</span>}
						{turnCostStr && <span>{turnCostStr}</span>}
					</div>
				)}

				{/* Turn-level message actions — always visible across all display modes */}
				{responseText && (
					<MessageActions>
						<MessageAction tooltip="Scroll to top" onClick={handleScrollToTop}>
							<ArrowUpToLineIcon className="size-3" />
						</MessageAction>
						<MessageAction
							tooltip={copied ? "Copied" : "Copy response"}
							onClick={handleCopyResponse}
						>
							{copied ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
						</MessageAction>
					{onForkFromTurn && !working && (
						<MessageAction
							tooltip={forking ? "Forking..." : "Fork from here"}
							onClick={handleFork}
							disabled={forking}
						>
							<GitForkIcon className="size-3" />
						</MessageAction>
					)}
					{onRevertToMessage && !working && (
						<MessageAction tooltip="Undo from here" onClick={handleRevertHere}>
							<Undo2Icon className="size-3" />
						</MessageAction>
					)}
					</MessageActions>
				)}
			</div>
		)
	},
	(prev, next) => {
		if (!areTurnsEqual(prev.turn, next.turn)) return false
		if (prev.isLast !== next.isLast) return false
		if (prev.isWorking !== next.isWorking) return false
		if (prev.agent?.sessionId !== next.agent?.sessionId) return false
		if (prev.agent?.directory !== next.agent?.directory) return false
		if (prev.agent?.projectDirectory !== next.agent?.projectDirectory) return false
		if (prev.agent?.worktreePath !== next.agent?.worktreePath) return false
		if (prev.isConnected !== next.isConnected) return false
		if (prev.compactionStatus !== next.compactionStatus) return false
		if (prev.stepsExpanded !== next.stepsExpanded) return false
		if (
			pendingPermissionFingerprint(prev.pendingPermission) !==
			pendingPermissionFingerprint(next.pendingPermission)
		) {
			return false
		}
		// Skip reference comparison for callbacks - they close over stable values
		// and their identity changes don't affect rendered output
		return true
	},
)
