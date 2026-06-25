/**
 * Devo type definitions.
 *
 * Re-exports canonical types from @devo-ai/sdk where available,
 * and defines local aliases for convenience in the converter.
 */

// ─── Re-exports from SDK ─────────────────────────────────────────────
// These are the authoritative types generated from the Devo OpenAPI spec.

export type {
	// Agent (runtime, from server)
	Agent as DevoAgent,
	// Agent (config-time)
	AgentConfig as DevoAgentConfig,
	AgentPart as DevoAgentPart,
	AssistantMessage as DevoAssistantMessage,
	// Commands
	Command as DevoCommand,
	CompactionPart as DevoCompactionPart,
	// Top-level config
	Config as DevoConfig,
	// Events (v1.2.0+)
	EventMessagePartDelta as DevoPartDelta,
	FilePart as DevoFilePart,
	// MCP
	McpLocalConfig as DevoMcpLocal,
	McpOAuthConfig as DevoMcpOAuth,
	McpRemoteConfig as DevoMcpRemote,
	Message as DevoMessage,
	// Models
	Model as DevoModel,
	Part as DevoPart,
	PatchPart as DevoPatchPart,
	// Permissions (runtime format)
	PermissionAction,
	PermissionActionConfig as DevoPermissionAction,
	// Permissions (config-time format)
	PermissionConfig as DevoPermission,
	PermissionObjectConfig as DevoPermissionObject,
	PermissionRule,
	PermissionRuleConfig as DevoPermissionRule,
	PermissionRuleset,
	Provider as DevoProvider,
	// Provider
	ProviderConfig as DevoProviderConfig,
	ReasoningPart as DevoReasoningPart,
	RetryPart as DevoRetryPart,
	// Server config
	ServerConfig as DevoServerConfig,
	// Session / Messages / Parts
	Session as DevoSession,
	SnapshotPart as DevoSnapshotPart,
	StepFinishPart as DevoStepFinishPart,
	StepStartPart as DevoStepStartPart,
	SubtaskPart as DevoSubtaskPart,
	TextPart as DevoTextPart,
	ToolPart as DevoToolPart,
	UserMessage as DevoUserMessage,
} from "@devo-ai/sdk/v2/client"

// ─── Local convenience types ─────────────────────────────────────────
// These are for converter internals only -- types that don't come from the SDK.

/** MCP config union (matches SDK's Config.mcp values) */
export type DevoMcp =
	| import("@devo-ai/sdk/v2/client").McpLocalConfig
	| import("@devo-ai/sdk/v2/client").McpRemoteConfig
	| { enabled: boolean }

/** Agent markdown frontmatter for .devo/agents/*.md */
export interface DevoAgentFrontmatter {
	description?: string
	mode?: "subagent" | "primary" | "all"
	model?: string
	temperature?: number
	color?: string
	steps?: number
	permission?: import("@devo-ai/sdk/v2/client").PermissionConfig
	hidden?: boolean
}

/** Command markdown frontmatter for .devo/commands/*.md */
export interface DevoCommandFrontmatter {
	description?: string
	agent?: string
	model?: string
	subtask?: boolean
}
