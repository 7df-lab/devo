/**
 * Rules file converter.
 *
 * CLAUDE.md -> AGENTS.md conversion.
 * Devo reads CLAUDE.md as a fallback, but AGENTS.md is preferred.
 */
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

export interface RulesConversionInput {
	/** CLAUDE.md content from project root */
	claudeMd?: string
	claudeMdPath?: string
	/** Existing AGENTS.md content if present */
	agentsMd?: string
	agentsMdPath?: string
	/** Project path for context */
	projectPath: string
}

export interface RulesConversionResult {
	/** AGENTS.md content to write (if needed) */
	agentsMd?: string
	report: MigrationReport
}

/**
 * Convert CLAUDE.md to AGENTS.md format.
 */
export function convertRules(input: RulesConversionInput): RulesConversionResult {
	const report = createEmptyReport()

	// If AGENTS.md already exists, skip (Devo prefers AGENTS.md over CLAUDE.md)
	if (input.agentsMd) {
		report.skipped.push({
			category: "rules",
			source: input.claudeMdPath ?? "CLAUDE.md",
			target: input.agentsMdPath ?? "AGENTS.md",
			details: "AGENTS.md already exists -- Devo will use it. CLAUDE.md kept as CC fallback.",
		})
		return { report }
	}

	// If no CLAUDE.md exists, nothing to do
	if (!input.claudeMd) {
		report.skipped.push({
			category: "rules",
			source: "No CLAUDE.md found",
			target: "",
		})
		return { report }
	}

	// Convert CLAUDE.md content to AGENTS.md
	// The content is the same -- it's just a rename with an optional header
	const agentsMd = input.claudeMd

	report.migrated.push({
		category: "rules",
		source: input.claudeMdPath ?? "CLAUDE.md",
		target: "AGENTS.md",
		details:
			"Content copied from CLAUDE.md. Original CLAUDE.md preserved for Claude Code compatibility.",
	})

	report.manualActions.push(
		`Consider renaming CLAUDE.md to AGENTS.md in your project. ` +
			`Devo reads both, but AGENTS.md is the preferred convention. ` +
			`If you use both Claude Code and Devo, keep CLAUDE.md and let Devo read it as fallback.`,
	)

	return { agentsMd, report }
}
