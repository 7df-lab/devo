/**
 * Command file converter.
 *
 * Claude Code: .claude/commands/*.md with {name, description}
 * Devo: .devo/commands/*.md with {description, agent, subtask}
 */

import type { DevoCommandFrontmatter } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import type { CommandFile } from "../types/scan-result"
import { serializeFrontmatter } from "../utils/yaml"

export interface CommandConversionResult {
	/** Map of target filename -> converted markdown content */
	commands: Map<string, string>
	report: MigrationReport
}

/**
 * Convert Claude Code command definitions to Devo format.
 */
export function convertCommands(commands: CommandFile[]): CommandConversionResult {
	const result = new Map<string, string>()
	const report = createEmptyReport()

	for (const cmd of commands) {
		try {
			const converted = convertSingleCommand(cmd)
			result.set(`${cmd.name}.md`, converted)
			report.migrated.push({
				category: "commands",
				source: cmd.path,
				target: `${cmd.name}.md`,
			})
		} catch (err) {
			report.errors.push(
				`Failed to convert command "${cmd.name}": ${err instanceof Error ? err.message : String(err)}`,
			)
		}
	}

	return { commands: result, report }
}

function convertSingleCommand(cmd: CommandFile): string {
	const fm = cmd.frontmatter
	const ocFm: DevoCommandFrontmatter = {}

	// Description
	ocFm.description = (fm.description as string) ?? cmd.name

	// Agent (default to build)
	ocFm.agent = "build"

	// Subtask (default to false)
	ocFm.subtask = false

	// Check if the command body uses shell execution patterns
	// that might benefit from the !`command` syntax in Devo
	if (cmd.body.includes("```bash") || cmd.body.includes("```sh")) {
		// Note in frontmatter isn't needed, but we could flag in report
	}

	return serializeFrontmatter(ocFm as unknown as Record<string, unknown>, cmd.body)
}
