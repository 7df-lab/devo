/**
 * Hooks -> Plugin stub converter.
 *
 * Claude Code hooks (PreToolUse, PostToolUse, etc.) -> Devo plugin stubs.
 * This generates TypeScript plugin files that replicate hook behavior.
 */
import type { ClaudeHookEntry, ClaudeHooks } from "../types/claude-code"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"

export interface HookConversionResult {
	/** Map of plugin file path -> TypeScript content */
	plugins: Map<string, string>
	report: MigrationReport
}

/**
 * Convert Claude Code hooks to Devo plugin stubs.
 */
export function convertHooks(hooks?: ClaudeHooks): HookConversionResult {
	const plugins = new Map<string, string>()
	const report = createEmptyReport()

	if (!hooks) return { plugins, report }

	const hookEntries: { type: string; entries: ClaudeHookEntry[] }[] = []

	for (const [type, entries] of Object.entries(hooks)) {
		if (Array.isArray(entries) && entries.length > 0) {
			hookEntries.push({ type, entries: entries as ClaudeHookEntry[] })
		}
	}

	if (hookEntries.length === 0) return { plugins, report }

	// Generate a single plugin file with all hook handlers
	const pluginCode = generatePluginCode(hookEntries)
	plugins.set("cc-hooks.ts", pluginCode)

	for (const { type, entries } of hookEntries) {
		report.migrated.push({
			category: "hooks",
			source: `hooks.${type} (${entries.length} entries)`,
			target: "plugins/cc-hooks.ts",
			details: "Generated plugin stub -- review and customize",
		})
	}

	report.manualActions.push(
		"Hook plugin stubs were generated in .devo/plugins/cc-hooks.ts. " +
			"Review the generated code and customize as needed. " +
			"Some hooks may not have exact Devo equivalents.",
	)

	return { plugins, report }
}

function generatePluginCode(hookEntries: { type: string; entries: ClaudeHookEntry[] }[]): string {
	const sections: string[] = []

	for (const { type, entries } of hookEntries) {
		for (const entry of entries) {
			for (const action of entry.hooks) {
				if (action.type === "command") {
					sections.push(generateCommandHook(type, entry.matcher, action.command))
				}
			}
		}
	}

	return `/**
 * Auto-generated plugin from Claude Code hooks.
 * Review and customize this file as needed.
 *
 * Claude Code hooks converted to Devo plugin hooks:
 * - PreToolUse / PostToolUse -> tool.execute.before / tool.execute.after
 * - UserPromptSubmit -> chat.message
 * - SessionStart -> (run on plugin load)
 * - Stop -> (no direct equivalent)
 */
import type { PluginInput } from "@devo-ai/plugin"

export default async (input: PluginInput) => {
	const { $ } = input

	return {
${sections.join("\n\n")}
	}
}
`
}

function generateCommandHook(
	hookType: string,
	matcher: string | undefined,
	command: string,
): string {
	const matcherComment = matcher ? ` // Matcher: ${matcher}` : ""
	const escapedCommand = command.replace(/\\/g, "\\\\").replace(/`/g, "\\`").replace(/\$/g, "\\$")

	switch (hookType) {
		case "PreToolUse":
			return `\t\t// PreToolUse hook${matcherComment}
\t\t"tool.execute.before": async (ctx) => {
\t\t\t${matcher ? `if (!/${matcher}/.test(ctx.tool)) return` : "// Runs for all tools"}
\t\t\ttry {
\t\t\t\tawait $\`${escapedCommand}\`
\t\t\t} catch (e) {
\t\t\t\tconsole.error("Pre-tool hook failed:", e)
\t\t\t}
\t\t},`

		case "PostToolUse":
			return `\t\t// PostToolUse hook${matcherComment}
\t\t"tool.execute.after": async (ctx) => {
\t\t\t${matcher ? `if (!/${matcher}/.test(ctx.tool)) return` : "// Runs for all tools"}
\t\t\ttry {
\t\t\t\tawait $\`${escapedCommand}\`
\t\t\t} catch (e) {
\t\t\t\tconsole.error("Post-tool hook failed:", e)
\t\t\t}
\t\t},`

		case "UserPromptSubmit":
			return `\t\t// UserPromptSubmit hook${matcherComment}
\t\t"chat.message": async (msg) => {
\t\t\ttry {
\t\t\t\tawait $\`${escapedCommand}\`
\t\t\t} catch (e) {
\t\t\t\tconsole.error("Message hook failed:", e)
\t\t\t}
\t\t\treturn msg
\t\t},`

		default:
			return `\t\t// ${hookType} hook (no direct Devo equivalent)${matcherComment}
\t\t// Original command: ${command}
\t\t// TODO: Implement manually`
	}
}
