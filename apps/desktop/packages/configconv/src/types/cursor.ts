/**
 * Type definitions for Cursor IDE configuration files.
 *
 * Derived from real-world ~/.cursor/ analysis and Cursor documentation.
 *
 * Cursor uses:
 * - `.cursor/rules/*.mdc` -- Project rules (MDC format: YAML frontmatter + markdown)
 * - `.cursor/mcp.json` / `~/.cursor/mcp.json` -- MCP server configuration
 * - `.cursor/commands/*.md` -- Slash commands (plain markdown)
 * - `.cursor/agents/*.md` -- Subagent definitions (YAML frontmatter + markdown)
 * - `.cursor/skills/` / `~/.cursor/skills/` -- Agent skills (SKILL.md)
 * - `~/.cursor/cli-config.json` -- CLI agent permissions
 * - `.cursorrules` -- Legacy single-file rules (deprecated)
 */

// ============================================================
// MCP Configuration
// ============================================================

/** .cursor/mcp.json or ~/.cursor/mcp.json */
export interface CursorMcpJson {
	mcpServers?: Record<string, CursorMcpServer>
}

/** Individual MCP server entry */
export interface CursorMcpServer {
	/** Command for stdio servers */
	command?: string
	/** Arguments for stdio servers */
	args?: string[]
	/** Environment variables */
	env?: Record<string, string>
	/** Path to .env file (stdio only) */
	envFile?: string
	/** Explicit type: "stdio" for local servers */
	type?: "stdio"
	/** URL for remote servers (SSE/HTTP) */
	url?: string
	/** HTTP headers for remote servers */
	headers?: Record<string, string>
	/** OAuth configuration */
	auth?: CursorOAuth
}

/** OAuth configuration for Cursor MCP servers */
export interface CursorOAuth {
	CLIENT_ID: string
	CLIENT_SECRET?: string
	scopes?: string[]
}

// ============================================================
// Rules (MDC format)
// ============================================================

/** Parsed .cursor/rules/*.mdc file */
export interface CursorRule {
	/** Absolute file path */
	path: string
	/** Filename without extension */
	name: string
	/** Raw file content */
	content: string
	/** Parsed YAML frontmatter */
	frontmatter: CursorRuleFrontmatter
	/** Markdown body (below frontmatter) */
	body: string
}

/** Frontmatter fields for .mdc rule files */
export interface CursorRuleFrontmatter {
	/** Description for "Apply Intelligently" mode */
	description?: string | null
	/** Glob patterns for file-scoped rules (comma-separated or array) */
	globs?: string | string[] | null
	/** If true, rule is always injected into every conversation */
	alwaysApply?: boolean
}

/**
 * Cursor rule application mode, derived from frontmatter fields:
 * - "always": alwaysApply=true
 * - "file-scoped": globs is set, alwaysApply is false
 * - "intelligent": description is set, no globs, alwaysApply is false
 * - "manual": no description, no globs, alwaysApply is false (only @-mention)
 */
export type CursorRuleMode = "always" | "file-scoped" | "intelligent" | "manual"

// ============================================================
// CLI Config
// ============================================================

/** ~/.cursor/cli-config.json */
export interface CursorCliConfig {
	version?: number
	editor?: {
		vimMode?: boolean
	}
	hasChangedDefaultModel?: boolean
	permissions?: CursorPermissions
}

/** Cursor permission format (similar to Claude Code) */
export interface CursorPermissions {
	allow?: string[]
	deny?: string[]
}

// ============================================================
// Agent definitions
// ============================================================

/** Parsed .cursor/agents/*.md file */
export interface CursorAgentFile {
	/** Absolute file path */
	path: string
	/** Filename without extension */
	name: string
	/** Raw file content */
	content: string
	/** Parsed YAML frontmatter */
	frontmatter: CursorAgentFrontmatter
	/** Markdown body */
	body: string
}

/** Frontmatter for Cursor agent .md files */
export interface CursorAgentFrontmatter {
	/** Agent name */
	name?: string
	/** Description: when to delegate to this agent */
	description?: string
}

// ============================================================
// Command definitions
// ============================================================

/** Parsed .cursor/commands/*.md file */
export interface CursorCommandFile {
	/** Absolute file path */
	path: string
	/** Filename without extension */
	name: string
	/** Raw file content */
	content: string
	/** Markdown body (Cursor commands are plain markdown, no frontmatter) */
	body: string
}

// ============================================================
// Skill definitions
// ============================================================

/** Cursor skill info (same SKILL.md format as other tools) */
export interface CursorSkillInfo {
	/** Absolute path to SKILL.md */
	path: string
	/** Skill directory name */
	name: string
	/** Description from frontmatter */
	description?: string
	/** Whether this is a symlink */
	isSymlink: boolean
	/** Symlink target if applicable */
	symlinkTarget?: string
}

// ============================================================
// Scan result (Cursor-specific)
// ============================================================

export interface CursorScanResult {
	global: CursorGlobalScanResult
	projects: CursorProjectScanResult[]
	/** Chat history (populated when includeHistory is true) */
	history?: CursorHistoryScanResult
}

export interface CursorGlobalScanResult {
	/** ~/.cursor/mcp.json */
	mcpJson?: CursorMcpJson
	mcpJsonPath?: string
	/** ~/.cursor/cli-config.json */
	cliConfig?: CursorCliConfig
	cliConfigPath?: string
	/** User-level skills */
	skills: CursorSkillInfo[]
	/** User-level commands */
	commands: CursorCommandFile[]
	/** User-level agents */
	agents: CursorAgentFile[]
}

export interface CursorProjectScanResult {
	/** Absolute path to the project */
	path: string
	/** .cursor/mcp.json */
	mcpJson?: CursorMcpJson
	mcpJsonPath?: string
	/** .cursor/rules/*.mdc files */
	rules: CursorRule[]
	/** .cursor/agents/*.md files */
	agents: CursorAgentFile[]
	/** .cursor/commands/*.md files */
	commands: CursorCommandFile[]
	/** .cursor/skills/ */
	skills: CursorSkillInfo[]
	/** AGENTS.md at project root */
	agentsMd?: string
	agentsMdPath?: string
	/** .cursorrules legacy file */
	cursorRules?: string
	cursorRulesPath?: string
}

/**
 * Determine the rule application mode from frontmatter fields.
 */
export function determineCursorRuleMode(fm: CursorRuleFrontmatter): CursorRuleMode {
	if (fm.alwaysApply === true) return "always"
	if (fm.globs && fm.globs !== "") return "file-scoped"
	if (fm.description && fm.description !== "") return "intelligent"
	return "manual"
}

// ============================================================
// Chat History Types
// ============================================================

/**
 * A discovered Cursor workspace (one hash directory in workspaceStorage/).
 * Each workspace maps to a single project and has its own state.vscdb.
 */
export interface CursorWorkspace {
	/** Hash directory name */
	hash: string
	/** Absolute path to the workspace hash directory */
	path: string
	/** Absolute path to the project (decoded from workspace.json folder URI) */
	projectPath?: string
	/** state.vscdb path */
	stateDbPath: string
	/** Number of composer tabs (chats) in this workspace */
	composerCount: number
}

/**
 * Metadata for a single Cursor composer tab (one chat conversation).
 * Extracted from workspace state.vscdb -> ItemTable -> composer.composerData -> allComposers[].
 */
export interface CursorComposerMeta {
	/** Unique composer ID (UUID) */
	composerId: string
	/** Chat title/name (may be empty for untitled chats) */
	name?: string
	/** Unix timestamp (ms) when the chat was created */
	createdAt?: number
	/** Unix timestamp (ms) when the chat was last updated */
	lastUpdatedAt?: number
	/** Chat mode: "chat", "agent", "edit", etc. */
	unifiedMode?: string
	/** Whether this chat is archived */
	isArchived?: boolean
	/** Total lines added during this chat */
	totalLinesAdded?: number
	/** Total lines removed during this chat */
	totalLinesRemoved?: number
}

/**
 * Full conversation data for a single Cursor composer tab.
 * Extracted from globalStorage state.vscdb -> cursorDiskKV -> composerData:<composerId>.
 */
export interface CursorComposerData {
	/** Schema version */
	_v?: number
	/** Composer ID */
	composerId: string
	/** Rich text content */
	richText?: string
	/** Plain text content */
	text?: string
	/** Ordered list of bubble headers (conversation turns) */
	fullConversationHeadersOnly: CursorBubbleHeader[]
	/** Model configuration used for this conversation */
	modelConfig?: CursorModelConfig
	/** Chat mode: "chat", "agent", "edit" */
	unifiedMode?: string
	/** When the conversation was created (unix ms) */
	createdAt?: number
	/** Conversation status */
	status?: string
}

/** Header for a single bubble in the conversation order. */
export interface CursorBubbleHeader {
	/** Bubble ID (UUID) */
	bubbleId: string
	/** 1 = user, 2 = assistant */
	type: 1 | 2
	/** Server-assigned bubble ID (assistant messages only) */
	serverBubbleId?: string
}

/** Model configuration for a composer. */
export interface CursorModelConfig {
	/** Model name (e.g., "claude-4-sonnet-1m-thinking", "gpt-4o") */
	modelName?: string
	/** Whether "max mode" was enabled */
	maxMode?: boolean
}

/**
 * Full bubble data (a single user or assistant message).
 * Stored in globalStorage state.vscdb -> cursorDiskKV -> bubbleId:<composerId>:<bubbleId>.
 */
export interface CursorBubble {
	/** Schema version */
	_v?: number
	/** 1 = user, 2 = assistant */
	type: 1 | 2
	/** Bubble ID */
	bubbleId: string
	/** Plain text content of the message */
	text?: string
	/** Rich text content (Lexical JSON) */
	richText?: string
	/** Tool call results (for agent mode) */
	toolResults?: CursorToolResult[]
	/** Thinking/reasoning blocks */
	allThinkingBlocks?: CursorThinkingBlock[]
	/** Token usage */
	tokenCount?: { inputTokens?: number; outputTokens?: number }
	/** Whether this was an agentic interaction */
	isAgentic?: boolean
	/** Code block data referenced in the message */
	codeBlockData?: Record<string, unknown>
	/** Suggested code edits */
	suggestedCodeBlocks?: unknown[]
	/** Attached file references */
	attachedFileCodeChunksMetadataOnly?: CursorAttachedFile[]
}

/** Tool result from an agent interaction. */
export interface CursorToolResult {
	toolName?: string
	result?: string
	error?: string
	[key: string]: unknown
}

/** Thinking/reasoning block from a model response. */
export interface CursorThinkingBlock {
	thinking?: string
	[key: string]: unknown
}

/** Attached file reference in a bubble. */
export interface CursorAttachedFile {
	relativeWorkspacePath?: string
	startLineNumber?: number
	intent?: number
	[key: string]: unknown
}

// ============================================================
// History Scan Result (Cursor-specific)
// ============================================================

/**
 * Result of scanning Cursor chat history across all workspaces.
 */
export interface CursorHistoryScanResult {
	/** Discovered workspaces */
	workspaces: CursorWorkspace[]
	/** All composer sessions with their conversations */
	sessions: CursorHistorySession[]
	/** Total number of composers found */
	totalSessions: number
	/** Total number of messages (bubbles) found */
	totalMessages: number
}

/**
 * A single chat session with resolved messages, ready for conversion.
 */
export interface CursorHistorySession {
	/** Composer ID */
	composerId: string
	/** Chat title */
	title: string
	/** Project path this chat belongs to */
	projectPath: string
	/** When created (unix ms) */
	createdAt: number
	/** When last updated (unix ms) */
	lastUpdatedAt: number
	/** Chat mode */
	mode: string
	/** Model used */
	model?: string
	/** Conversation messages in order */
	messages: CursorHistoryMessage[]
}

/** A resolved message in a Cursor chat session. */
export interface CursorHistoryMessage {
	/** Bubble ID */
	bubbleId: string
	/** "user" or "assistant" */
	role: "user" | "assistant"
	/** Plain text content */
	text: string
	/** Tool results (if any) */
	toolResults?: CursorToolResult[]
	/** Thinking blocks (if any) */
	thinkingBlocks?: CursorThinkingBlock[]
	/** Token count */
	tokenCount?: { inputTokens?: number; outputTokens?: number }
}
