/**
 * Types for the converter output.
 */
import type { DevoConfig } from "./devo"
import type { MigrationReport } from "./report"

export type MigrationCategory =
	| "config"
	| "mcp"
	| "agents"
	| "commands"
	| "skills"
	| "permissions"
	| "rules"
	| "history"
	| "hooks"

export interface ConvertOptions {
	/** Which categories to convert (default: all except history) */
	categories?: MigrationCategory[]
	/** Include session history conversion */
	includeHistory?: boolean
	/** Manual model ID overrides */
	modelOverrides?: Record<string, string>
	/** Default model to use if none detected */
	defaultModel?: string
	/** Default small model */
	defaultSmallModel?: string
}

export interface ConversionResult {
	/** Global devo.json config to write */
	globalConfig: Partial<DevoConfig>
	/** Per-project devo.json configs */
	projectConfigs: Map<string, Partial<DevoConfig>>
	/** Agent files to write: target path -> markdown content */
	agents: Map<string, string>
	/** Command files to write: target path -> markdown content */
	commands: Map<string, string>
	/** Rules files to write: target path -> content */
	rules: Map<string, string>
	/** Hook plugin stubs: target path -> TypeScript content */
	hookPlugins: Map<string, string>
	/** Converted sessions (if history included) */
	sessions?: ConvertedSession[]
	/** Prompt history entries (if history included) */
	promptHistory?: ConvertedPromptEntry[]
	/** Full migration report */
	report: MigrationReport
}

export interface ConvertedSession {
	projectId: string
	session: {
		id: string
		slug: string
		version: string
		projectID: string
		directory: string
		title: string
		time: { created: number; updated: number }
		summary?: { additions: number; deletions: number; files: number }
	}
	messages: ConvertedMessage[]
}

export interface ConvertedMessage {
	id: string
	sessionID: string
	role: "user" | "assistant"
	time: { created: number; updated: number }
	parts: ConvertedPart[]
}

export interface ConvertedPart {
	id: string
	messageID: string
	type: "text" | "reasoning" | "tool-invocation" | "tool-result"
	content: string
}

export interface ConvertedPromptEntry {
	input: string
	parts: unknown[]
	mode: string
}
