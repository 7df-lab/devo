/**
 * Automation type definitions.
 *
 * These types are shared across the automation subsystem (registry,
 * scheduler, executor, IPC). Validation is handled by simple runtime
 * checks rather than Zod to avoid the dependency.
 */

// ============================================================
// Status and result types
// ============================================================

export type AutomationStatus = "active" | "paused" | "archived"

export type AutomationRunStatus =
	| "queued"
	| "running"
	| "pending_review"
	| "accepted"
	| "archived"
	| "failed"

export type EffortLevel = "low" | "medium" | "high"

export type ApprovalPolicy = "never" | "auto-edit"

/**
 * Permission presets control how the agent handles tool permissions:
 * - "default": Inherit project config, agent asks when needed
 * - "allow-all": Full autonomous mode, all permissions allowed
 * - "read-only": No file modifications allowed
 */
export type PermissionPreset = "default" | "allow-all" | "read-only"

// ============================================================
// Config types (stored on disk as JSON)
// ============================================================

export interface AutomationSchedule {
	rrule: string
	timezone: string
}

export interface ExecutionConfig {
	/** Model to use in "providerID/modelID" format (e.g. "anthropic/claude-opus-4-5"). Defaults to server default. */
	model?: string
	/** Agent name to use (e.g. "build", "research"). Defaults to server default agent. */
	agent?: string
	/** Model variant name (e.g. "extended" for extended thinking). Defaults to model default. */
	variant?: string
	effort: EffortLevel
	timeout: number
	retries: number
	retryDelay: number
	parallelWorkspaces: boolean
	approvalPolicy: ApprovalPolicy
	/** Whether to run in an isolated git worktree (default: true) */
	useWorktree: boolean
	/** Permission preset controlling agent tool access */
	permissionPreset: PermissionPreset
}

export interface AutomationConfig {
	version: 1
	name: string
	status: AutomationStatus
	schedule: AutomationSchedule
	workspaces: string[]
	execution: ExecutionConfig
}

export interface RunResult {
	title: string
	summary: string
	hasActionableOutput: boolean
	branchName: string | null
	prUrl: string | null
}

// ============================================================
// Runtime types (merged with DB state)
// ============================================================

export interface Automation {
	id: string
	name: string
	prompt: string
	status: AutomationStatus
	schedule: AutomationSchedule
	workspaces: string[]
	execution: ExecutionConfig
	nextRunAt: number | null
	lastRunAt: number | null
	runCount: number
	consecutiveFailures: number
	createdAt: number
	updatedAt: number
}

export interface AutomationRun {
	id: string
	automationId: string
	workspace: string
	status: AutomationRunStatus
	attempt: number
	sessionId: string | null
	worktreePath: string | null
	startedAt: number | null
	completedAt: number | null
	timeoutAt: number | null
	resultTitle: string | null
	resultSummary: string | null
	resultHasActionable: boolean | null
	resultBranch: string | null
	resultPrUrl: string | null
	errorMessage: string | null
	archivedReason: string | null
	archivedAssistantMessage: string | null
	readAt: number | null
	createdAt: number
	updatedAt: number
}

// ============================================================
// IPC payload types
// ============================================================

export interface CreateAutomationInput {
	name: string
	prompt: string
	schedule: { rrule: string; timezone?: string }
	workspaces: string[]
	execution?: Partial<ExecutionConfig>
}

export interface UpdateAutomationInput {
	id: string
	name?: string
	prompt?: string
	status?: AutomationStatus
	schedule?: { rrule: string; timezone?: string }
	workspaces?: string[]
	execution?: Partial<ExecutionConfig>
}

// ============================================================
// Defaults
// ============================================================

export const DEFAULT_EXECUTION_CONFIG: ExecutionConfig = {
	effort: "medium",
	timeout: 600,
	retries: 0,
	retryDelay: 60,
	parallelWorkspaces: false,
	approvalPolicy: "never",
	useWorktree: true,
	permissionPreset: "default",
}
