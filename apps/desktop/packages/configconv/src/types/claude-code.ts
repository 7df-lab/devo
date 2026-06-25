/**
 * Type definitions for Claude Code configuration files.
 *
 * Derived from real-world ~/.Claude/ and ~/.claude.json analysis,
 * validated against @anthropic-ai/claude-agent-sdk@0.2.39 (sdk.d.ts).
 *
 * NOTE: The SDK exports runtime API types (streaming messages, hook callbacks),
 * NOT config-file schema types. We must maintain these locally for:
 * - settings.json, .claude.json, .mcp.json file structures
 * - Agent/command markdown frontmatter
 * - Session JSONL transcript format
 *
 * The SDK DOES export these useful types we reference:
 * - PermissionMode: 'default' | 'acceptEdits' | 'bypassPermissions' | 'plan' | 'delegate' | 'dontAsk'
 * - HookEvent: 15 event names (PreToolUse, PostToolUse, etc.)
 * - HOOK_EVENTS: const array of event names
 */

/**
 * Permission mode for Claude Code.
 * Matches PermissionMode from @anthropic-ai/claude-agent-sdk.
 */
export type ClaudePermissionMode =
	| "default"
	| "acceptEdits"
	| "bypassPermissions"
	| "plan"
	| "delegate"
	| "dontAsk"

/**
 * Hook event names.
 * Matches HookEvent from @anthropic-ai/claude-agent-sdk.
 */
export type ClaudeHookEvent =
	| "PreToolUse"
	| "PostToolUse"
	| "PostToolUseFailure"
	| "Notification"
	| "UserPromptSubmit"
	| "SessionStart"
	| "SessionEnd"
	| "Stop"
	| "SubagentStart"
	| "SubagentStop"
	| "PreCompact"
	| "PermissionRequest"
	| "Setup"
	| "TeammateIdle"
	| "TaskCompleted"

/** ~/.Claude/settings.json */
export interface ClaudeSettings {
	env?: Record<string, string>
	permissions?: ClaudePermissions
	model?: string
	autoUpdatesChannel?: string
	teammateMode?: string
	hooks?: ClaudeHooks
	/** Sandboxing settings - can be boolean, string mode, or full config object */
	sandbox?: boolean | string | ClaudeSandboxSettings
	/** External API key helper command */
	apiKeyHelper?: string
	/** Days before cleaning up old sessions */
	cleanupPeriodDays?: number
	/** Include co-authored-by in git commits */
	includeCoAuthoredBy?: boolean
	/** Output style preference */
	outputStyle?: string
}

/**
 * Sandbox settings object.
 * Matches SandboxSettings from @anthropic-ai/claude-agent-sdk.
 */
export interface ClaudeSandboxSettings {
	enabled?: boolean
	/** Auto-allow bash if sandboxed */
	autoAllowBashIfSandboxed?: boolean
	/** Network configuration */
	network?: ClaudeSandboxNetworkConfig
}

export interface ClaudeSandboxNetworkConfig {
	/** Allow network access */
	allowNetworkAccess?: boolean
	/** Allowed hosts */
	allowedHosts?: string[]
	/** Ignore network violations for these patterns */
	ignoreViolations?: string[]
}

export interface ClaudePermissions {
	allow?: string[]
	deny?: string[]
	ask?: string[]
	/** Default permission mode. Matches PermissionMode from SDK. */
	defaultMode?: ClaudePermissionMode
	/** Whether to disable bypass permissions mode entirely */
	disableBypassPermissionsMode?: boolean
	/** Additional directories the model can access */
	additionalDirectories?: string[]
}

/**
 * Claude Code hooks configuration.
 * Keys are ClaudeHookEvent values from the SDK's HOOK_EVENTS const.
 * Values are arrays of hook entries with matchers and command actions.
 *
 * Note: The SDK's runtime hooks use HookCallback (async functions),
 * but the config file uses this JSON structure with command strings.
 */
export type ClaudeHooks = Partial<Record<ClaudeHookEvent, ClaudeHookEntry[]>>

export interface ClaudeHookEntry {
	matcher?: string
	hooks: ClaudeHookAction[]
}

export interface ClaudeHookAction {
	type: "command"
	command: string
}

/** ~/.claude.json -- global user state */
export interface ClaudeUserState {
	numStartups?: number
	installMethod?: string
	autoUpdates?: boolean
	theme?: string
	hasCompletedOnboarding?: boolean
	bypassPermissionsModeAccepted?: boolean
	hasOpusPlanDefault?: boolean
	projects?: Record<string, ClaudeProjectEntry>
	githubRepoPaths?: Record<string, string>
}

export interface ClaudeProjectEntry {
	allowedTools?: string[]
	mcpServers?: Record<string, ClaudeMcpServer>
	mcpContextUris?: string[]
	enabledMcpjsonServers?: string[]
	disabledMcpjsonServers?: string[]
	hasTrustDialogAccepted?: boolean
	ignorePatterns?: string[]
	exampleFiles?: string[]
	lastCost?: number
	lastSessionId?: string
	lastModelUsage?: Record<string, unknown>
}

/** MCP server config from claude.json or .mcp.json */
export interface ClaudeMcpServer {
	command?: string
	args?: string[]
	env?: Record<string, string>
	type?: "sse" | "http" | "stdio"
	url?: string
	headers?: Record<string, string>
	/** Platform-specific command overrides */
	platform_overrides?: Record<string, Partial<ClaudeMcpServer>>
}

/** .mcp.json project-level config */
export interface ClaudeMcpJson {
	mcpServers?: Record<string, ClaudeMcpServer>
}

/** .claude/settings.local.json */
export interface ClaudeProjectSettings {
	permissions?: ClaudePermissions
	env?: Record<string, string>
	model?: string
	mcpServers?: Record<string, ClaudeMcpServer>
}

/**
 * Agent markdown file frontmatter.
 * Full set of fields from Claude Code source.
 */
export interface ClaudeAgentFrontmatter {
	name?: string
	description?: string
	/** Tool list -- can be comma-separated string or array */
	tools?: string | string[]
	/** Tools to disallow */
	disallowedTools?: string[]
	/** Model: sonnet, opus, haiku, best, inherit, or full model ID */
	model?: string
	/** Permission mode override. Matches PermissionMode from SDK. */
	permissionMode?: ClaudePermissionMode
	/** MCP servers to enable for this agent */
	mcpServers?: string[]
	/** Hooks specific to this agent */
	hooks?: ClaudeHooks
	/** Max turns before stopping */
	maxTurns?: number
	/** Skills to load */
	skills?: string[]
	/** Memory scope: user, project, or local */
	memory?: "user" | "project" | "local"
	/** Agent color for UI display */
	color?: "red" | "blue" | "green" | "yellow" | "purple" | "orange" | "pink" | "cyan"
	/** Whether to fork context on subagent invocation */
	forkContext?: boolean
}

/** Command markdown file frontmatter */
export interface ClaudeCommandFrontmatter {
	name?: string
	description?: string
}

/** Session index entry from sessions-index.json */
export interface ClaudeSessionIndexEntry {
	sessionId: string
	fullPath: string
	fileMtime?: number
	firstPrompt?: string
	summary?: string
	messageCount?: number
	created?: string
	modified?: string
	gitBranch?: string
	projectPath?: string
	isSidechain?: boolean
}

/** Session index file */
export interface ClaudeSessionIndex {
	version: number
	entries: ClaudeSessionIndexEntry[]
	originalPath?: string
}

/** Single line from history.jsonl */
export interface ClaudeHistoryEntry {
	display: string
	pastedContents?: Record<string, unknown>
	timestamp: number
	project?: string
	sessionId?: string
}

/**
 * Single line from a session .jsonl transcript.
 *
 * The actual format uses `type` at the top level to distinguish line kinds,
 * with the Anthropic API message nested under `message`.
 * Non-message line types: "summary", "file-history-snapshot".
 */
export interface ClaudeSessionLine {
	type: "user" | "assistant" | "summary" | "file-history-snapshot"
	/** Anthropic API message (present for user/assistant lines) */
	message?: ClaudeSessionApiMessage
	/** Message UUID */
	uuid?: string
	/** Parent UUID (for threading) */
	parentUuid?: string
	/** Session ID */
	sessionId?: string
	/** Git branch at time of message */
	gitBranch?: string
	/** Working directory */
	cwd?: string
	/** Summary text (for type="summary" lines) */
	summary?: string
}

/** The Anthropic API message nested inside a session line */
export interface ClaudeSessionApiMessage {
	role: "user" | "assistant"
	content?: string | ClaudeContentBlock[]
	model?: string
	id?: string
	type?: string
}

/** A content block within an assistant message */
export interface ClaudeContentBlock {
	type: "text" | "thinking" | "tool_use" | "tool_result"
	text?: string
	thinking?: string
	/** For tool_use blocks */
	name?: string
	id?: string
	input?: unknown
	/** For tool_result blocks */
	tool_use_id?: string
	content?: string | unknown[]
}
