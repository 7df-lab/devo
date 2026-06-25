/**
 * Permission system converter.
 *
 * Claude Code: trust-based with allow/deny/ask lists of "ToolName(pattern)" strings
 * Devo: granular per-tool rules with allow/deny/ask actions and glob patterns
 */
import type { ClaudePermissions } from "../types/claude-code"
import type { DevoPermissionAction } from "../types/devo"

/**
 * Internal mutable permission map.
 * We build this during conversion and cast to the SDK's PermissionConfig at the end.
 * The SDK type has specific named keys but also allows arbitrary string keys via index signature.
 */
type PermissionMap = Record<
	string,
	DevoPermissionAction | Record<string, DevoPermissionAction>
>

import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

/** Tool name mapping: Claude Code -> Devo */
const TOOL_NAME_MAP: Record<string, string> = {
	Read: "read",
	Write: "write",
	Edit: "edit",
	MultiEdit: "edit",
	Bash: "bash",
	Glob: "glob",
	Grep: "grep",
	WebFetch: "webfetch",
	WebSearch: "websearch",
	Task: "task",
	TodoRead: "todoread",
	TodoWrite: "todowrite",
	Skill: "skill",
}

/** Parse a Claude Code tool pattern like "Bash(git *)" */
export interface ParsedToolPattern {
	toolName: string
	pattern: string
}

export function parseToolPattern(raw: string): ParsedToolPattern | null {
	const match = raw.match(/^(\w+)\((.+)\)$/)
	if (match) {
		return { toolName: match[1], pattern: match[2] }
	}
	// No pattern -- just tool name
	if (/^\w+$/.test(raw)) {
		return { toolName: raw, pattern: "*" }
	}
	return null
}

/**
 * Map a Claude Code tool name to an Devo permission key.
 */
export function mapToolName(ccToolName: string): string | null {
	return TOOL_NAME_MAP[ccToolName] ?? null
}

export interface PermissionConversionResult {
	permission: PermissionMap
	report: MigrationReport
}

/**
 * Convert Claude Code permissions to Devo permission format.
 */
export function convertPermissions(
	ccPermissions?: ClaudePermissions,
	allowedTools?: string[],
): PermissionConversionResult {
	const report = createEmptyReport()
	const permission: PermissionMap = {}

	if (!ccPermissions && !allowedTools) {
		return { permission: { "*": "ask" }, report }
	}

	// Step 1: Determine default mode
	if (ccPermissions?.defaultMode === "bypassPermissions") {
		permission["*"] = "allow"
		report.migrated.push({
			category: "permissions",
			source: 'defaultMode: "bypassPermissions"',
			target: '"*": "allow"',
		})
	} else {
		permission["*"] = "ask"
	}

	// Step 2: Process allow list
	const allowEntries = ccPermissions?.allow ?? []
	processToolPatterns(allowEntries, "allow", permission, report)

	// Step 3: Process deny list
	const denyEntries = ccPermissions?.deny ?? []
	processToolPatterns(denyEntries, "deny", permission, report)

	// Step 4: Process ask list (rarely used)
	const askEntries = ccPermissions?.ask ?? []
	processToolPatterns(askEntries, "ask", permission, report)

	// Step 5: Process per-project allowedTools
	if (allowedTools) {
		processToolPatterns(allowedTools, "allow", permission, report)
	}

	// Step 6: Simplify rules
	simplifyPermissions(permission)

	return { permission, report }
}

function processToolPatterns(
	patterns: string[],
	action: DevoPermissionAction,
	permission: PermissionMap,
	report: MigrationReport,
): void {
	for (const raw of patterns) {
		const parsed = parseToolPattern(raw)
		if (!parsed) {
			report.warnings.push(`Could not parse tool pattern: "${raw}"`)
			continue
		}

		const ocTool = mapToolName(parsed.toolName)
		if (!ocTool) {
			report.warnings.push(`Unknown tool name "${parsed.toolName}" in pattern "${raw}". Skipped.`)
			continue
		}

		if (parsed.pattern === "*") {
			// Simple wildcard -- set tool to action directly
			const existing = permission[ocTool]
			if (typeof existing === "object") {
				// Already has patterns -- add wildcard
				existing["*"] = action
			} else {
				permission[ocTool] = action
			}
		} else {
			// Pattern-specific -- need nested object
			const existing = permission[ocTool]
			if (typeof existing === "object") {
				existing[parsed.pattern] = action
			} else if (typeof existing === "string") {
				// Promote simple action to object
				permission[ocTool] = {
					"*": existing,
					[parsed.pattern]: action,
				}
			} else {
				permission[ocTool] = { [parsed.pattern]: action }
			}
		}

		report.migrated.push({
			category: "permissions",
			source: raw,
			target: `permission.${ocTool}${parsed.pattern !== "*" ? `["${parsed.pattern}"]` : ""} = "${action}"`,
		})
	}
}

/**
 * Simplify permission rules where possible.
 * If an object only has "*": action, collapse to just action.
 */
function simplifyPermissions(permission: PermissionMap): void {
	for (const [key, value] of Object.entries(permission)) {
		if (typeof value === "object") {
			const entries = Object.entries(value)
			if (entries.length === 1 && entries[0][0] === "*") {
				permission[key] = entries[0][1]
			}
		}
	}
}

/**
 * Convert a Claude Code tool list (from agent frontmatter) to Devo permissions.
 * Example: "Read, Edit, Bash, Grep" -> { read: "allow", edit: "allow", bash: "ask", ... }
 */
export function convertToolListToPermissions(toolList: string): PermissionMap {
	const tools = toolList
		.split(",")
		.map((t) => t.trim())
		.filter(Boolean)

	const permission: PermissionMap = {}

	for (const tool of tools) {
		const ocTool = mapToolName(tool)
		if (ocTool) {
			// Bash defaults to "ask" for safety; everything else "allow"
			permission[ocTool] = ocTool === "bash" ? "ask" : "allow"
		}
	}

	return permission
}
