import {
	CodeBlock,
	CodeBlockActions,
	CodeBlockContent,
	CodeBlockCopyButton,
	CodeBlockHeader,
	CodeBlockTitle,
} from "@devo/ui/components/ai-elements/code-block"
import { Diff, DiffContent } from "@devo/ui/components/ai-elements/diff"
import {
	Terminal,
	TerminalContent,
	TerminalCopyButton,
	TerminalHeader,
	TerminalTitle,
} from "@devo/ui/components/ai-elements/terminal"
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@devo/ui/components/dialog"
import { cn } from "@devo/ui/lib/utils"

import { useSetAtom } from "jotai"
import {
	AlertTriangleIcon,
	BookOpenIcon,
	CodeIcon,
	DiffIcon,
	EditIcon,
	EyeIcon,
	FileCodeIcon,
	FileIcon,
	GlobeIcon,
	Loader2Icon,
	MinusIcon,
	PlusIcon,
	SearchIcon,
	SquareCheckIcon,
	TerminalIcon,
	WrenchIcon,
	XIcon,
	ZapIcon,
} from "lucide-react"
import type { ReactNode } from "react"
import { memo, useCallback, useMemo } from "react"
import { useToolElapsedTime } from "../../hooks/use-elapsed-time"
import type { BundledLanguage } from "shiki"
import { viewFileInDiffPanelAtom } from "../../atoms/ui"
import { detectContentLanguage, detectLanguage, prettyPrintJson } from "../../lib/language"
import type { FilePart, ToolPart, ToolStateCompleted } from "../../lib/types"
import { SubAgentCard } from "./sub-agent-card"
import { getToolCategory, ToolCard } from "./tool-card"

// ============================================================
// Constants
// ============================================================

/** Max characters to display in tool output before truncating */
const MAX_OUTPUT_LENGTH = 5000

/** Truncate output for display, preserving useful content */
function truncateOutput(output: string, max = MAX_OUTPUT_LENGTH): string {
	if (output.length <= max) return output
	return `${output.slice(0, max)}\n... (truncated)`
}

// ============================================================
// Read output parsing — strip cat -n prefixes and <file> tags
// ============================================================

/** Pre-compiled regex for line-number formats (hoisted to avoid re-creation per call) */
const LINE_NUM_REGEX = /^\s*(\d+)[|:\t]\s?(.*)$/

/**
 * Parses output from various read tools.
 * Handles:
 * 1. Claude Code's `cat -n` format: <file>00001| content</file>
 * 2. Devo's XML-wrapped format: <path>...</path><content>1: content</content>
 *
 * This function strips the wrapper tags, removes the line-number
 * prefixes, and returns the clean content + the starting line number.
 */
function parseReadOutput(raw: string): { content: string; startLine: number } {
	let text = raw

	// 1. Strip Devo XML-style tags
	text = text.replace(/<path>[\s\S]*?<\/path>\s*\n?/g, "")
	text = text.replace(/<type>[\s\S]*?<\/type>\s*\n?/g, "")

	// Extract content from <content> or <entries> if present
	const contentMatch = text.match(/<content>([\s\S]*?)<\/content>/)
	const entriesMatch = text.match(/<entries>([\s\S]*?)<\/entries>/)

	if (contentMatch) {
		text = contentMatch[1]
	} else if (entriesMatch) {
		text = entriesMatch[1]
	} else {
		// 2. Fallback: Strip <file> / </file> wrapper lines (Claude Code)
		text = text.replace(/^\s*<file>\s*\n?/, "")
		text = text.replace(/\n?\s*<\/file>\s*$/, "")
	}

	// 3. Clean up trailing metadata lines
	text = text.replace(/\n?\s*\(End of file[^)]*\)\s*$/, "")
	text = text.replace(/\n?\s*\(File has more lines[^)]*\)\s*$/, "")
	text = text.replace(/\n?\s*\(Output truncated[^)]*\)\s*$/, "")

	const lines = text.split("\n")
	// If first line has a line number, use it as startLine
	const firstMatch = lines[0]?.match(LINE_NUM_REGEX)

	if (!firstMatch) {
		return { content: text, startLine: 1 }
	}

	const startLine = Number.parseInt(firstMatch[1], 10)
	const stripped: string[] = []

	for (const line of lines) {
		const match = line.match(LINE_NUM_REGEX)
		if (match) {
			stripped.push(match[2])
		} else {
			// Lines without prefix (e.g. blank lines) — keep as-is
			stripped.push(line)
		}
	}

	return { content: stripped.join("\n"), startLine }
}

// ============================================================
// Diff stats computation
// ============================================================

/**
 * Computes +additions / -deletions stats from old/new strings.
 */
function computeDiffStats(
	oldStr: string,
	newStr: string,
): { additions: number; deletions: number } {
	const oldLines = oldStr.split("\n")
	const newLines = newStr.split("\n")
	const oldSet = new Set(oldLines)
	const newSet = new Set(newLines)

	let additions = 0
	let deletions = 0

	for (const line of newLines) {
		if (!oldSet.has(line)) additions++
	}
	for (const line of oldLines) {
		if (!newSet.has(line)) deletions++
	}

	return { additions, deletions }
}

// ============================================================
// Tool info resolver
// ============================================================

export function getToolInfo(tool: string): {
	icon: typeof WrenchIcon
	title: string
} {
	switch (tool) {
		case "read":
			return { icon: EyeIcon, title: "Read" }
		case "glob":
			return { icon: SearchIcon, title: "Glob" }
		case "grep":
			return { icon: SearchIcon, title: "Grep" }
		case "list":
			return { icon: SearchIcon, title: "List" }
		case "webfetch":
			return { icon: GlobeIcon, title: "Fetch" }
		case "bash":
			return { icon: TerminalIcon, title: "Shell" }
		case "edit":
			return { icon: EditIcon, title: "Edit" }
		case "write":
			return { icon: FileCodeIcon, title: "Write" }
		case "apply_patch":
			return { icon: CodeIcon, title: "Patch" }
		case "task":
			return { icon: ZapIcon, title: "Agent" }
		case "todowrite":
			return { icon: SquareCheckIcon, title: "Todos" }
		case "todoread":
			return { icon: SquareCheckIcon, title: "Todos" }
		case "question":
			return { icon: BookOpenIcon, title: "Question" }
		default:
			return { icon: WrenchIcon, title: tool }
	}
}

/**
 * Try to extract a field value from partial JSON in `state.raw`.
 * During the `pending` state, `input` may be `{}` while the server is still
 * streaming the tool-call arguments. The `raw` field (when available) contains
 * the accumulated partial JSON string, so we can attempt to pull out early
 * fields like `command` or `description` even before the server has finished
 * parsing the full input.
 */
function extractFromRaw(state: ToolPart["state"], ...fields: string[]): string | undefined {
	if (!("raw" in state) || typeof state.raw !== "string" || !state.raw) return undefined
	const raw = state.raw
	for (const field of fields) {
		// Match "field": "value" — handles escaped quotes in the value
		const pattern = new RegExp(`"${field}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"`)
		const match = raw.match(pattern)
		if (match?.[1]) return match[1]
	}
	return undefined
}

/**
 * Returns a "Preparing ..." fallback label for tools in the `pending` state
 * when no meaningful subtitle could be resolved from input/raw yet.
 * Mirrors the Devo TUI's `InlineTool` behaviour (e.g. "~ Preparing write ...").
 */
function getPendingLabel(tool: string): string {
	switch (tool) {
		case "write":
			return "Preparing write..."
		case "edit":
			return "Preparing edit..."
		case "apply_patch":
			return "Preparing patch..."
		case "bash":
			return "Preparing command..."
		case "read":
			return "Preparing read..."
		case "task":
			return "Preparing agent..."
		case "webfetch":
			return "Preparing fetch..."
		default:
			return `Preparing ${tool}...`
	}
}

/**
 * Extracts a human-readable subtitle from tool state.
 * Falls back to a "Preparing ..." label when the tool is in the `pending` state
 * and no input fields have been parsed yet (the model is still streaming arguments).
 */
export function getToolSubtitle(part: ToolPart): string | undefined {
	const state = part.state
	const input = state.input
	const rawTitle = "title" in state && typeof state.title === "string" ? state.title : undefined
	const title = rawTitle && rawTitle !== part.tool && rawTitle !== part.callID ? rawTitle : undefined

	let subtitle: string | undefined

	switch (part.tool) {
		case "read":
			subtitle =
				shortenPath((input.filePath as string) ?? (input.path as string)) ??
				shortenPath(extractFromRaw(state, "filePath", "path"))
			break
		case "glob":
			subtitle =
				(input.pattern as string) ??
				(input.path as string) ??
				extractFromRaw(state, "pattern", "path")
			break
		case "grep":
			subtitle =
				(input.pattern as string) ??
				(input.path as string) ??
				extractFromRaw(state, "pattern", "path")
			break
		case "bash":
			subtitle =
				title ??
				(input.description as string) ??
				(input.command as string) ??
				extractFromRaw(state, "command", "description")
			break
		case "edit":
			subtitle =
				shortenPath((input.filePath as string) ?? (input.path as string)) ??
				shortenPath(extractFromRaw(state, "filePath", "path"))
			break
		case "write":
			subtitle =
				shortenPath((input.filePath as string) ?? (input.path as string)) ??
				shortenPath(extractFromRaw(state, "filePath", "path"))
			break
		case "apply_patch":
			subtitle = title
			break
		case "webfetch":
			subtitle = (input.url as string) ?? extractFromRaw(state, "url")
			break
		case "task":
			subtitle = (input.description as string) ?? title ?? extractFromRaw(state, "description")
			break
		case "todowrite":
		case "todoread": {
			const todos = input?.todos as Array<{ status: string }> | undefined
			if (todos && todos.length > 0) {
				const completed = todos.filter((t) => t.status === "completed").length
				subtitle = `${completed}/${todos.length} completed`
			} else {
				subtitle = title
			}
			break
		}
		default:
			// Unknown / MCP tools: always show compact input params like [key=value, key=value]
			// Input params are more useful than the SDK-generated title for MCP tools
			subtitle = formatInputParams(input) ?? title
			break
	}

	// When pending with no resolved subtitle, show a "Preparing ..." label so
	// the user sees activity instead of a blank card (matches Devo TUI behaviour).
	if (!subtitle && state.status === "pending") {
		return getPendingLabel(part.tool)
	}

	return subtitle
}

/**
 * Formats tool input as a compact bracket notation for unknown/MCP tools.
 * e.g. { url: "https://...", format: "md" } → [url=https://..., format=md]
 */
function formatInputParams(input: Record<string, unknown>): string | undefined {
	const entries = Object.entries(input)
	if (entries.length === 0) return undefined

	const parts: string[] = []
	for (const [key, value] of entries) {
		if (value == null) continue
		const strVal = typeof value === "string" ? value : JSON.stringify(value)
		// Truncate long values
		const truncated = strVal.length > 60 ? `${strVal.slice(0, 57)}...` : strVal
		parts.push(`${key}=${truncated}`)
	}

	if (parts.length === 0) return undefined
	return `[${parts.join(", ")}]`
}

/** Shorten a file path to just filename or last 2 segments */
function shortenPath(path: string | undefined): string | undefined {
	if (!path) return undefined
	const parts = path.split("/")
	if (parts.length <= 2) return path
	return parts.slice(-2).join("/")
}

/** Compute completed tool duration from the SDK tool-state timestamps. */
export function getToolDuration(part: ToolPart): string | undefined {
	const state = part.state
	if (state.status === "completed" || state.status === "error") {
		const ms = Math.max(0, state.time.end - state.time.start)
		if (ms < 1000) return `${ms}ms`
		const seconds = Math.floor(ms / 1000)
		if (seconds < 60) return `${seconds}s`
		const minutes = Math.floor(seconds / 60)
		const remainingSeconds = seconds % 60
		return `${minutes}m ${remainingSeconds}s`
	}
	return undefined
}

// ============================================================
// Tool-specific content renderers
// ============================================================

/**
 * Bash tool: shows command with syntax highlighting + ANSI-colored terminal output.
 *
 * During `running` state the server streams incremental output via
 * `state.metadata.output` (accumulated string, updated on every stdout/stderr
 * chunk). We read that field so the terminal updates in real-time, matching the
 * behaviour of the Devo TUI and web UI.
 */
function BashContent({ part }: { part: ToolPart }) {
	const command = part.state.input?.command as string | undefined

	// During "running", live output arrives in state.metadata.output.
	// After completion it moves to state.output.
	const streamingOutput =
		part.state.status === "running"
			? (part.state.metadata?.output as string | undefined)
			: undefined
	const output = part.state.status === "completed" ? part.state.output : streamingOutput
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined
	const isStreaming = part.state.status === "running"

	const displayOutput = useMemo(() => {
		if (error) return error
		if (!output) return ""
		let cleaned = output
		// The SDK output often echoes the command as the first line (e.g. "$ command\n...").
		// Since we already render the command in a separate CodeBlock above, strip the duplicate.
		if (command) {
			const prefix = `$ ${command}`
			if (cleaned.startsWith(prefix)) {
				cleaned = cleaned.slice(prefix.length).replace(/^\r?\n/, "")
			}
		}
		return truncateOutput(cleaned)
	}, [output, error, command])

	// If there's meaningful output, show a terminal view with ANSI support
	if (displayOutput || isStreaming) {
		return (
			<div className="space-y-1.5">
				{command && (
					<CodeBlock
						code={`$ ${command}`}
						language="bash"
						className="border-0 shadow-none rounded-none text-[11px]"
					>
						<CodeBlockContent code={`$ ${command}`} language="bash" />
					</CodeBlock>
				)}
				<Terminal
					output={displayOutput}
					isStreaming={isStreaming}
					className={cn(
						"max-h-64 border-0 shadow-none rounded-none text-[11px]",
						error && "border-red-500/30",
					)}
				>
					<TerminalHeader className="py-1.5 px-3">
						<TerminalTitle className="text-[11px]">Output</TerminalTitle>
						<TerminalCopyButton className="size-6" />
					</TerminalHeader>
					<TerminalContent className="max-h-48 p-3 text-[11px] leading-relaxed" />
				</Terminal>
			</div>
		)
	}

	// Command only (no output yet)
	if (command) {
		return (
			<CodeBlock
				code={`$ ${command}`}
				language="bash"
				className="border-0 shadow-none rounded-none text-[11px]"
			>
				<CodeBlockContent code={`$ ${command}`} language="bash" />
			</CodeBlock>
		)
	}

	return null
}

/**
 * Edit tool: shows inline diff of oldString → newString with syntax highlighting.
 * No inner headers — diff stats are shown in the ToolCard trailing area.
 */
function EditDiffContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const oldString = part.state.input?.oldString as string | undefined
	const newString = part.state.input?.newString as string | undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	// If we have both old and new strings, show a proper diff
	if (oldString != null && newString != null) {
		const fileName = filePath?.split("/").pop() ?? "file"
		return (
			<Diff
				mode="files"
				oldFile={{ name: fileName, content: oldString }}
				newFile={{ name: fileName, content: newString }}
				className="max-h-96 text-[11px] border-0 shadow-none rounded-none"
			>
				<DiffContent maxHeight={384} showLineNumbers hideFileHeader />
			</Diff>
		)
	}

	// Fallback: just show the output text
	const output = part.state.status === "completed" ? part.state.output : undefined
	if (output) {
		return (
			<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
				<code>{truncateOutput(output)}</code>
			</pre>
		)
	}

	return null
}

/**
 * Write tool: shows syntax-highlighted file content being written.
 * No inner header — the ToolCard header already shows the filename.
 */
function WriteContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const content = part.state.input?.content as string | undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	const language = (detectLanguage(filePath) ?? "text") as BundledLanguage

	if (content) {
		const displayContent = truncateOutput(content)
		return (
			<CodeBlock
				code={displayContent}
				language={language}
				showLineNumbers
				className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
			>
				<CodeBlockContent code={displayContent} language={language} showLineNumbers />
			</CodeBlock>
		)
	}

	// Fallback to output
	const output = part.state.status === "completed" ? part.state.output : undefined
	if (output) {
		return (
			<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
				<code>{truncateOutput(output)}</code>
			</pre>
		)
	}

	return null
}

/**
 * Apply patch tool: shows diff in patch mode.
 * No inner header — the ToolCard header identifies this as a patch.
 */
function PatchContent({ part }: { part: ToolPart }) {
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	// Try to detect if output is a patch/diff
	if (output) {
		const trimmed = output.trimStart()
		const isPatch =
			trimmed.startsWith("---") || trimmed.startsWith("diff --git") || trimmed.startsWith("@@")

		if (isPatch) {
			return (
				<Diff
					mode="patch"
					patch={output}
					className="max-h-96 text-[11px] border-0 shadow-none rounded-none"
				>
					<DiffContent maxHeight={384} showLineNumbers hideFileHeader />
				</Diff>
			)
		}

		// Not a patch format — show as plain diff-highlighted text
		return (
			<CodeBlock
				code={truncateOutput(output)}
				language="diff"
				className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
			>
				<CodeBlockContent code={truncateOutput(output)} language="diff" />
			</CodeBlock>
		)
	}

	return null
}

/**
 * Read tool: shows syntax-highlighted file contents.
 * Strips cat -n prefixes and <file> tags, preserves original line numbers.
 */
function ReadContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />
	if (!output) return null

	const language = (detectLanguage(filePath) ?? "text") as BundledLanguage

	// Parse out cat -n line numbers and <file> wrapper
	const { content: cleanContent, startLine } = parseReadOutput(output)
	const displayContent = truncateOutput(cleanContent)

	// No inner header — the ToolCard header already shows the filename.
	// Just the code block with correct line number offset.
	return (
		<CodeBlock
			code={displayContent}
			language={language}
			showLineNumbers
			className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
		>
			<CodeBlockContent
				code={displayContent}
				language={language}
				showLineNumbers
				startLine={startLine}
			/>
		</CodeBlock>
	)
}

/** Search tools (glob/grep/list): shows pattern + results */
function SearchContent({ part }: { part: ToolPart }) {
	const pattern = (part.state.input?.pattern as string) ?? undefined
	const include = (part.state.input?.include as string) ?? undefined
	const path = (part.state.input?.path as string) ?? undefined
	const output = part.state.status === "completed" ? part.state.output : undefined

	return (
		<div className="space-y-1.5 px-3.5 py-2.5">
			<div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground/70">
				{pattern && (
					<span>
						pattern: <span className="font-mono text-foreground/60">{pattern}</span>
					</span>
				)}
				{include && (
					<span>
						include: <span className="font-mono text-foreground/60">{include}</span>
					</span>
				)}
				{path && (
					<span>
						in: <span className="font-mono text-foreground/60">{shortenPath(path)}</span>
					</span>
				)}
			</div>
			{output && (
				<pre className="max-h-48 overflow-auto rounded bg-muted/40 px-2 py-1 font-mono text-[11px] text-muted-foreground">
					<code>{truncateOutput(output)}</code>
				</pre>
			)}
		</div>
	)
}

/** WebFetch tool: shows URL + fetched content with optional markdown/json highlighting */
function WebFetchContent({ part }: { part: ToolPart }) {
	const url = part.state.input?.url as string | undefined
	const format = part.state.input?.format as string | undefined
	const output = part.state.status === "completed" ? part.state.output : undefined

	const language = useMemo(() => {
		if (format === "html") return "html"
		if (format === "json") return "json"
		if (output) return detectContentLanguage(output)
		return undefined
	}, [format, output]) as BundledLanguage | undefined

	const displayOutput = useMemo(() => {
		if (!output) return undefined
		if (language === "json") return prettyPrintJson(output)
		return output
	}, [output, language])

	return (
		<div className="space-y-1.5">
			{url && (
				<div className="truncate px-3.5 pt-2.5 font-mono text-xs text-muted-foreground/70">
					{url}
				</div>
			)}
			{displayOutput && language ? (
				<CodeBlock
					code={truncateOutput(displayOutput)}
					language={language}
					className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
				>
					<CodeBlockContent code={truncateOutput(displayOutput)} language={language} />
				</CodeBlock>
			) : output ? (
				<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
					<code>{truncateOutput(output)}</code>
				</pre>
			) : null}
		</div>
	)
}

/** TodoWrite tool: shows checklist items */
function TodoContent({ part }: { part: ToolPart }) {
	const todos =
		(part.state.input?.todos as Array<{ content: string; status: string }> | undefined) ?? []

	if (todos.length === 0) return null

	return (
		<div className="space-y-1 px-3.5 py-2.5">
			{todos.map((todo, i) => (
				<div
					key={`todo-${todo.content.slice(0, 20)}-${i}`}
					className="flex items-start gap-2 text-xs"
				>
					<span className="mt-0.5">
						{todo.status === "completed" ? (
							<SquareCheckIcon className="size-3.5 text-green-500" />
						) : todo.status === "in_progress" ? (
							<Loader2Icon className="size-3.5 animate-spin text-blue-400" />
						) : todo.status === "cancelled" ? (
							<SquareCheckIcon className="size-3.5 text-muted-foreground/40" />
						) : (
							<span className="inline-block size-3.5 rounded-sm border border-border" />
						)}
					</span>
					<span
						className={cn(
							todo.status === "completed"
								? "text-muted-foreground line-through"
								: todo.status === "cancelled"
									? "text-muted-foreground/50 line-through"
									: todo.status === "in_progress"
										? "text-foreground"
										: "text-foreground/80",
						)}
					>
						{todo.content}
					</span>
				</div>
			))}
		</div>
	)
}

/** Error content for any tool */
function ErrorContent({ error }: { error: string }) {
	return (
		<div className="flex items-start gap-2 rounded bg-red-500/5 mx-3.5 my-2.5 px-2 py-1.5 text-xs text-red-400">
			<AlertTriangleIcon className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
			<pre className="max-h-32 overflow-auto font-mono">
				<code>{error.length > 500 ? `${error.slice(0, 500)}...` : error}</code>
			</pre>
		</div>
	)
}

/**
 * Generic tool output: auto-detects JSON and other structured content
 * for syntax highlighting, falls back to plain text.
 */
function GenericContent({ part }: { part: ToolPart }) {
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	const language = useMemo(() => {
		if (!output) return undefined
		return detectContentLanguage(output) as BundledLanguage | undefined
	}, [output])

	const displayOutput = useMemo(() => {
		if (!output) return undefined
		if (language === "json") return prettyPrintJson(output)
		return output
	}, [output, language])

	return (
		<div>
			{displayOutput && language ? (
				<CodeBlock
					code={truncateOutput(displayOutput)}
					language={language}
					className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
				>
					<CodeBlockHeader className="px-3 py-1.5">
						<CodeBlockTitle className="text-[11px]">
							<span className="uppercase text-muted-foreground">{language}</span>
						</CodeBlockTitle>
						<CodeBlockActions>
							<CodeBlockCopyButton className="size-6" />
						</CodeBlockActions>
					</CodeBlockHeader>
					<CodeBlockContent code={truncateOutput(displayOutput)} language={language} />
				</CodeBlock>
			) : output ? (
				<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
					<code>{truncateOutput(output)}</code>
				</pre>
			) : null}
			{error && <ErrorContent error={error} />}
		</div>
	)
}

// ============================================================
// Smart defaults: which tools should be open by default
// ============================================================

/**
 * Returns whether a tool should default to expanded in the active turn.
 */
export function shouldDefaultOpen(_tool: string, status: string): boolean {
	// Errors stay expanded so failure output remains visible for recovery.
	return status === "error"
}
/**
 * Returns whether a tool has expandable content.
 */
function hasExpandableContent(part: ToolPart): boolean {
	const { tool, state } = part
	// Task uses SubAgentCard, not ToolCard
	if (tool === "task") return false
	// Todowrite has expandable todo items
	if (tool === "todowrite" || tool === "todoread") {
		const todos = state.input?.todos as Array<{ content: string; status: string }> | undefined
		return (todos?.length ?? 0) > 0
	}
	// Edit tool has content if we have old/new strings
	if (tool === "edit") {
		const oldString = state.input?.oldString as string | undefined
		const newString = state.input?.newString as string | undefined
		if (oldString != null && newString != null) return true
	}
	// Write tool has content if we have the file content
	if (tool === "write") {
		const content = state.input?.content as string | undefined
		if (content) return true
	}
	// If there's output or error, there's content
	if (state.status === "completed" && state.output) return true
	if (state.status === "error") return true
	// Bash always has content (the command at least)
	if (tool === "bash") return true
	return false
}

/**
 * Resolves the content renderer for a tool.
 */
function getToolContent(part: ToolPart): ReactNode {
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined
	if (
		error &&
		part.tool !== "bash" &&
		part.tool !== "edit" &&
		part.tool !== "write" &&
		part.tool !== "read" &&
		part.tool !== "apply_patch"
	) {
		return <ErrorContent error={error} />
	}

	switch (part.tool) {
		case "bash":
			return <BashContent part={part} />
		case "edit":
			return <EditDiffContent part={part} />
		case "write":
			return <WriteContent part={part} />
		case "apply_patch":
			return <PatchContent part={part} />
		case "read":
			return <ReadContent part={part} />
		case "glob":
		case "grep":
		case "list":
			return <SearchContent part={part} />
		case "webfetch":
			return <WebFetchContent part={part} />
		case "todowrite":
		case "todoread":
			return <TodoContent part={part} />
		default:
			return <GenericContent part={part} />
	}
}

// ============================================================
// ChatToolCall — main export
// ============================================================

interface ChatToolCallProps {
	part: ToolPart
	/** Whether this tool is in the active (last) turn */
	isActiveTurn?: boolean
	/** Whether the turn containing this tool has an error (enables delete action) */
	turnHasError?: boolean
	/** Delete this tool part (for error recovery) */
	onDelete?: (part: ToolPart) => void
}

/**
 * Compares two ToolPart objects for meaningful changes.
 * Avoids re-renders when a new object reference has the same content.
 */
function areToolPartsEqual(a: ToolPart, b: ToolPart): boolean {
	if (a === b) return true
	if (a.id !== b.id) return false
	if (a.state.status !== b.state.status) return false
	// During "pending", the server streams partial tool-call arguments into
	// state.raw. Compare raw length so the subtitle updates as arguments arrive.
	if (a.state.status === "pending" && b.state.status === "pending") {
		if (a.state.raw.length !== b.state.raw.length) return false
	}
	// During "running", the server streams incremental output via
	// state.metadata.output. Compare the accumulated output length so
	// React re-renders the terminal on every new chunk.
	if (a.state.status === "running" && b.state.status === "running") {
		const aMeta = a.state.metadata?.output as string | undefined
		const bMeta = b.state.metadata?.output as string | undefined
		if ((aMeta?.length ?? 0) !== (bMeta?.length ?? 0)) return false
	}
	// Compare output/error lengths for completed/error states
	if (a.state.status === "completed" && b.state.status === "completed") {
		if (a.state.output.length !== b.state.output.length) return false
		if (a.state.time.end !== b.state.time.end) return false
	}
	if (a.state.status === "error" && b.state.status === "error") {
		if (a.state.error !== b.state.error) return false
	}
	return true
}

/**
 * Renders a single tool call as a ToolCard with tool-specific content,
 * or as a SubAgentCard for sub-agent tasks.
 */
export const ChatToolCall = memo(
	function ChatToolCall({
		part,
		isActiveTurn = false,
		turnHasError = false,
		onDelete,
	}: ChatToolCallProps) {
		const viewFileInDiff = useSetAtom(viewFileInDiffPanelAtom)

		// Compute diff stats for edit tools (shown in trailing area)
		// Must be called before early returns to satisfy hooks rules.
		const diffStats = useMemo(() => {
			if (part.tool !== "edit") return undefined
			const oldString = part.state.input?.oldString as string | undefined
			const newString = part.state.input?.newString as string | undefined
			if (oldString == null || newString == null) return undefined
			return computeDiffStats(oldString, newString)
		}, [part.tool, part.state.input?.oldString, part.state.input?.newString])

		// Extract file path for edit-category tools (used for "View diff" button)
		const editFilePath = useMemo(() => {
			if (part.tool !== "edit" && part.tool !== "write" && part.tool !== "apply_patch") {
				return undefined
			}
			return (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
		}, [part.tool, part.state.input?.filePath, part.state.input?.path])

		const handleViewDiff = useCallback(
			(e: React.MouseEvent) => {
				e.stopPropagation()
				if (editFilePath) viewFileInDiff(editFilePath)
			},
			[editFilePath, viewFileInDiff],
		)

		const duration = getToolDuration(part)
		const elapsedTime = useToolElapsedTime(part)
		const status = part.state.status as "running" | "error" | "completed" | "pending"

		// Build trailing element: diff stats + "view diff" button + duration/spinner
		// Must be called before early returns to satisfy hooks rules.
		const trailingElement = useMemo(() => {
			const parts: ReactNode[] = []

			// Diff stats for edit tools
			if (diffStats) {
				parts.push(
					<span key="stats" className="flex items-center gap-1.5 text-[11px]">
						<span className="flex items-center gap-0.5 text-diff-addition-foreground">
							<PlusIcon className="size-2.5" aria-hidden="true" />
							{diffStats.additions}
						</span>
						<span className="flex items-center gap-0.5 text-diff-deletion-foreground">
							<MinusIcon className="size-2.5" aria-hidden="true" />
							{diffStats.deletions}
						</span>
					</span>,
				)
			}

			// Duration, elapsed time + spinner, or just spinner
			if (duration) {
				parts.push(
					<span key="duration" className="text-[11px]">
						{duration}
					</span>,
				)
			} else if (status === "running" || status === "pending") {
				parts.push(
					<span key="running" className="flex items-center gap-1.5">
						{elapsedTime && (
							<span className="text-[11px] tabular-nums text-muted-foreground/50">
								{elapsedTime}
							</span>
						)}
						<Loader2Icon className="size-3 animate-spin text-muted-foreground/40" />
					</span>,
				)
			}

			if (parts.length === 0) return undefined
			if (parts.length === 1) return parts[0]
			return <span className="flex items-center gap-2.5">{parts}</span>
		}, [diffStats, duration, elapsedTime, status])

		// Combine trailing element with "View diff" button for edit-category tools
		const combinedTrailing = useMemo(() => {
			if (!editFilePath || status !== "completed") return trailingElement
			const viewButton = (
				<button
					key="view-diff"
					type="button"
					onClick={handleViewDiff}
					className="rounded p-0.5 text-muted-foreground/40 transition-colors hover:bg-muted hover:text-foreground"
					title="View in diff panel"
				>
					<DiffIcon className="size-3" aria-hidden="true" />
				</button>
			)
			if (!trailingElement) return viewButton
			return (
				<span className="flex items-center gap-2">
					{trailingElement}
					{viewButton}
				</span>
			)
		}, [editFilePath, status, trailingElement, handleViewDiff])

		// When the turn has an error, add a delete button so the user can
		// surgically remove a problematic tool part and continue the conversation.
		const handleDelete = useCallback(
			(e: React.MouseEvent) => {
				e.stopPropagation()
				onDelete?.(part)
			},
			[onDelete, part],
		)

		const finalTrailing = useMemo(() => {
			if (!turnHasError || !onDelete) return combinedTrailing
			const deleteButton = (
				<button
					key="delete-part"
					type="button"
					onClick={handleDelete}
					className="rounded p-0.5 text-muted-foreground/40 transition-colors hover:bg-red-500/20 hover:text-red-400"
					title="Remove this tool call to recover from the error"
				>
					<XIcon className="size-3" aria-hidden="true" />
				</button>
			)
			if (!combinedTrailing) return deleteButton
			return (
				<span className="flex items-center gap-2">
					{combinedTrailing}
					{deleteButton}
				</span>
			)
		}, [turnHasError, onDelete, combinedTrailing, handleDelete])

		// Skip rendering todoread parts without output
		if (part.tool === "todoread" && part.state.status !== "completed") return null

		// --- Task tool: Sub-agent card ---
		if (part.tool === "task") {
			return <SubAgentCard part={part} />
		}

		// --- All other tools (including todos): ToolCard ---
		const { icon: Icon, title } = getToolInfo(part.tool)
		const subtitle = getToolSubtitle(part)
		const category = getToolCategory(part.tool)
		const hasContent = hasExpandableContent(part)
		const defaultOpen = isActiveTurn ? shouldDefaultOpen(part.tool, status) : false

		// Extract attachments
		const attachments: FilePart[] =
			part.state.status === "completed"
				? ((part.state as ToolStateCompleted).attachments ?? [])
				: []

		return (
			<div className="space-y-1.5">
				<ToolCard
					icon={<Icon className="size-3.5" />}
					title={title}
					subtitle={subtitle}
					trailing={finalTrailing}
					category={category}
					defaultOpen={defaultOpen}
					forceOpen={status === "error"}
					hasContent={hasContent}
					status={status}
					renderContent={hasContent ? () => getToolContent(part) : undefined}
				/>

				{/* Tool attachments (images, etc.) */}
				{attachments.length > 0 && <ToolAttachments attachments={attachments} />}
			</div>
		)
	},
	(prev, next) => {
		if (!areToolPartsEqual(prev.part, next.part)) return false
		if (prev.isActiveTurn !== next.isActiveTurn) return false
		if (prev.turnHasError !== next.turnHasError) return false
		// onDelete is a callback ref - skip reference comparison to avoid
		// re-renders from parent creating new closures
		return true
	},
)

// ============================================================
// ToolAttachments — inline thumbnails for tool output images
// ============================================================

function ToolAttachments({ attachments }: { attachments: FilePart[] }) {
	const imageAttachments = attachments.filter((a) => a.mime.startsWith("image/"))
	const otherAttachments = attachments.filter((a) => !a.mime.startsWith("image/"))

	if (imageAttachments.length === 0 && otherAttachments.length === 0) return null

	return (
		<div className="ml-6 flex flex-wrap gap-2">
			{imageAttachments.map((file) => (
				<Dialog key={file.id}>
					<DialogTrigger
						render={
							<button
								type="button"
								className="group/att relative size-12 shrink-0 overflow-hidden rounded border border-border bg-muted transition-colors hover:border-muted-foreground/30"
							/>
						}
					>
						<img
							src={file.url}
							alt={file.filename ?? "Tool output image"}
							className="size-full object-cover"
						/>
					</DialogTrigger>
					<DialogContent className="max-h-[90vh] max-w-4xl overflow-auto p-0">
						<DialogTitle className="sr-only">{file.filename ?? "Tool output preview"}</DialogTitle>
						<img
							src={file.url}
							alt={file.filename ?? "Tool output image"}
							className="max-h-[85vh] w-full object-contain"
						/>
					</DialogContent>
				</Dialog>
			))}
			{otherAttachments.map((file) => (
				<div
					key={file.id}
					className="flex items-center gap-1 rounded border border-border bg-muted px-2 py-1 text-[11px] text-muted-foreground"
				>
					<FileIcon className="size-3" aria-hidden="true" />
					<span className="max-w-[120px] truncate">{file.filename ?? file.mime}</span>
				</div>
			))}
		</div>
	)
}
