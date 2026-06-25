/**
 * Canonical intermediate representation for agent configuration.
 *
 * All format-specific types (Claude Code, Devo, Cursor) convert
 * to/from this canonical form. This enables N-to-N conversion:
 *   Source format -> Canonical -> Target format
 *
 * The canonical types are tool-agnostic and represent the "ideal"
 * configuration shape that captures the union of all supported features.
 */

// ============================================================
// Format identifiers
// ============================================================

export type AgentFormat = "claude-code" | "devo" | "cursor"

// ============================================================
// Top-level scan result (format-agnostic)
// ============================================================

export interface CanonicalScanResult {
	/** Which format this was scanned from */
	sourceFormat: AgentFormat
	/** Global (user-level) configuration */
	global: CanonicalGlobalConfig
	/** Per-project configurations */
	projects: CanonicalProjectConfig[]
}

// ============================================================
// Global configuration
// ============================================================

export interface CanonicalGlobalConfig {
	/** Primary model identifier (format-agnostic, e.g. "claude-opus-4-6") */
	model?: string
	/** Small/fast model for titles, summaries, etc. */
	smallModel?: string
	/** Provider hint (e.g. "anthropic", "amazon-bedrock") */
	provider?: string
	/** MCP server configurations */
	mcpServers: Record<string, CanonicalMcpServer>
	/** Permission rules */
	permissions?: CanonicalPermissions
	/** Global rules/instructions content */
	rules?: CanonicalRulesFile[]
	/** Skills */
	skills: CanonicalSkillInfo[]
	/** Custom slash commands */
	commands: CanonicalCommandFile[]
	/** Agent definitions */
	agents: CanonicalAgentFile[]
	/** Arbitrary environment variables */
	env?: Record<string, string>
	/** Auto-update preference */
	autoUpdate?: boolean
	/** Raw format-specific settings that don't map cleanly */
	extraSettings?: Record<string, unknown>
}

// ============================================================
// Project configuration
// ============================================================

export interface CanonicalProjectConfig {
	/** Absolute path to the project root */
	path: string
	/** Project-level model override */
	model?: string
	/** MCP server configurations */
	mcpServers: Record<string, CanonicalMcpServer>
	/** Permission rules */
	permissions?: CanonicalPermissions
	/** Rules/instructions files */
	rules: CanonicalRulesFile[]
	/** Skills */
	skills: CanonicalSkillInfo[]
	/** Custom slash commands */
	commands: CanonicalCommandFile[]
	/** Agent definitions */
	agents: CanonicalAgentFile[]
	/** Disabled MCP server names */
	disabledMcpServers?: string[]
	/** Enabled MCP server names */
	enabledMcpServers?: string[]
	/** File ignore patterns */
	ignorePatterns?: string[]
	/** Raw format-specific project settings */
	extraSettings?: Record<string, unknown>
}

// ============================================================
// MCP Servers
// ============================================================

export interface CanonicalMcpServer {
	/** Server type */
	type: "local" | "remote"
	/** For local: executable command */
	command?: string
	/** For local: command arguments */
	args?: string[]
	/** For local: environment variables */
	env?: Record<string, string>
	/** For remote: server URL (SSE or HTTP) */
	url?: string
	/** For remote: HTTP headers */
	headers?: Record<string, string>
	/** Whether the server is enabled */
	enabled?: boolean
	/** OAuth configuration (if any) */
	oauth?: Record<string, unknown>
}

// ============================================================
// Permissions
// ============================================================

/**
 * Canonical permission model.
 * Maps tool names to permission actions, with optional glob sub-patterns.
 *
 * Examples:
 *   { "*": "allow" }
 *   { "*": "ask", "bash": { "git *": "allow", "*": "ask" }, "read": "allow" }
 */
export type CanonicalPermissions = Record<
	string,
	CanonicalPermissionAction | Record<string, CanonicalPermissionAction>
>

export type CanonicalPermissionAction = "allow" | "deny" | "ask"

// ============================================================
// Rules (instructions files)
// ============================================================

export interface CanonicalRulesFile {
	/** Absolute file path */
	path: string
	/** File name */
	name: string
	/** Raw content */
	content: string
	/** Whether this rule should always be applied */
	alwaysApply?: boolean
	/** File glob patterns this rule applies to */
	globs?: string
	/** Description for intelligent/auto-apply rules */
	description?: string
	/** The source format's rule type hint */
	ruleType?: "always" | "file-scoped" | "intelligent" | "manual" | "general"
}

// ============================================================
// Agent definitions
// ============================================================

export interface CanonicalAgentFile {
	/** Absolute file path */
	path: string
	/** Filename without extension */
	name: string
	/** Raw file content */
	content: string
	/** Parsed frontmatter */
	frontmatter: Record<string, unknown>
	/** Markdown body (below frontmatter) */
	body: string
	/** Agent description */
	description?: string
	/** Agent mode: primary (top-level) or subagent (delegated) */
	mode?: "primary" | "subagent"
	/** Model override */
	model?: string
	/** Tool/permission restrictions */
	tools?: string[]
	/** Temperature override */
	temperature?: number
	/** Max steps/turns */
	maxSteps?: number
	/** Agent color for UI */
	color?: string
}

// ============================================================
// Command definitions
// ============================================================

export interface CanonicalCommandFile {
	/** Absolute file path */
	path: string
	/** Filename without extension (used as /command-name) */
	name: string
	/** Raw file content */
	content: string
	/** Parsed frontmatter */
	frontmatter: Record<string, unknown>
	/** Markdown body (below frontmatter) */
	body: string
	/** Command description */
	description?: string
}

// ============================================================
// Skills
// ============================================================

export interface CanonicalSkillInfo {
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
// Conversion result (format-agnostic output)
// ============================================================

export interface CanonicalConversionResult {
	/** Source format */
	sourceFormat: AgentFormat
	/** Target format */
	targetFormat: AgentFormat
	/** Global config to write */
	globalConfig: Record<string, unknown>
	/** Per-project configs: project path -> config object */
	projectConfigs: Map<string, Record<string, unknown>>
	/** Agent files to write: target path -> markdown content */
	agents: Map<string, string>
	/** Command files to write: target path -> markdown content */
	commands: Map<string, string>
	/** Rules files to write: target path -> content */
	rules: Map<string, string>
	/** Additional files to write (plugins, etc.): target path -> content */
	extraFiles: Map<string, string>
	/** Full migration report */
	report: ConversionReport
}

// ============================================================
// Conversion report
// ============================================================

export interface ConversionReport {
	/** Successfully converted items */
	converted: ConversionReportItem[]
	/** Items that were skipped */
	skipped: ConversionReportItem[]
	/** Non-fatal warnings */
	warnings: string[]
	/** Actions the user must take manually */
	manualActions: string[]
	/** Errors encountered */
	errors: string[]
}

export interface ConversionReportItem {
	/** Item category */
	category: ConversionCategory
	/** Source description */
	source: string
	/** Target description */
	target: string
	/** Additional details */
	details?: string
}

export type ConversionCategory =
	| "config"
	| "mcp"
	| "agents"
	| "commands"
	| "skills"
	| "permissions"
	| "rules"
	| "hooks"
	| "history"

// ============================================================
// Helpers
// ============================================================

export function createEmptyReport(): ConversionReport {
	return {
		converted: [],
		skipped: [],
		warnings: [],
		manualActions: [],
		errors: [],
	}
}

export function mergeReports(...reports: ConversionReport[]): ConversionReport {
	return {
		converted: reports.flatMap((r) => r.converted),
		skipped: reports.flatMap((r) => r.skipped),
		warnings: reports.flatMap((r) => r.warnings),
		manualActions: reports.flatMap((r) => r.manualActions),
		errors: reports.flatMap((r) => r.errors),
	}
}

// ============================================================
// Convert options
// ============================================================

export interface UniversalConvertOptions {
	/** Source format */
	from: AgentFormat
	/** Target format */
	to: AgentFormat
	/** Which categories to convert (default: all) */
	categories?: ConversionCategory[]
	/** Manual model ID overrides */
	modelOverrides?: Record<string, string>
	/** Default model to use if none detected */
	defaultModel?: string
	/** Default small model */
	defaultSmallModel?: string
}
