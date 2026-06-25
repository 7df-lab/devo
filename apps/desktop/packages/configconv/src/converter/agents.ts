/**
 * Agent definition converter.
 *
 * Claude Code: .claude/agents/*.md with {name, description, tools, model}
 * Devo: .devo/agents/*.md with {description, mode, model, temperature, permission, ...}
 */

import type { DevoAgentFrontmatter } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import type { AgentFile } from "../types/scan-result"
import { serializeFrontmatter } from "../utils/yaml"
import { translateModelId } from "./model-id"
import { convertToolListToPermissions } from "./permissions"

export interface AgentConversionInput {
	agents: AgentFile[]
	provider?: "anthropic" | "amazon-bedrock" | "google-vertex"
	modelOverrides?: Record<string, string>
}

export interface AgentConversionResult {
	/** Map of target filename -> converted markdown content */
	agents: Map<string, string>
	report: MigrationReport
}

/**
 * Convert Claude Code agent definitions to Devo format.
 */
export function convertAgents(input: AgentConversionInput): AgentConversionResult {
	const agents = new Map<string, string>()
	const report = createEmptyReport()

	for (const agent of input.agents) {
		try {
			const converted = convertSingleAgent(agent, input.provider, input.modelOverrides)
			agents.set(`${agent.name}.md`, converted)
			report.migrated.push({
				category: "agents",
				source: agent.path,
				target: `${agent.name}.md`,
				details: `Converted frontmatter (mode, temperature, permissions)`,
			})
		} catch (err) {
			report.errors.push(
				`Failed to convert agent "${agent.name}": ${err instanceof Error ? err.message : String(err)}`,
			)
		}
	}

	return { agents, report }
}

function convertSingleAgent(
	agent: AgentFile,
	provider?: "anthropic" | "amazon-bedrock" | "google-vertex",
	modelOverrides?: Record<string, string>,
): string {
	const fm = agent.frontmatter
	const ocFm: DevoAgentFrontmatter = {}

	// Description (required in OC)
	ocFm.description = (fm.description as string) ?? agent.name

	// Mode inference
	ocFm.mode = inferMode(agent.name, fm.description as string | undefined)

	// Model translation
	const ccModel = fm.model as string | undefined
	if (ccModel && ccModel !== "inherit") {
		ocFm.model = translateModelId(ccModel, provider, modelOverrides)
	}
	// If "inherit" or missing, omit model (inherits from config)

	// Temperature inference based on agent purpose
	ocFm.temperature = inferTemperature(agent.name, fm.description as string | undefined)

	// Steps
	ocFm.steps = ocFm.mode === "subagent" ? 25 : 50

	// Convert tools list to permissions
	const toolList = fm.tools as string | undefined
	if (toolList) {
		ocFm.permission = convertToolListToPermissions(toolList)
	}

	return serializeFrontmatter(cleanUndefined(ocFm), agent.body)
}

/**
 * Infer agent mode from name and description.
 */
function inferMode(name: string, description?: string): "primary" | "subagent" {
	const text = `${name} ${description ?? ""}`.toLowerCase()

	// Keywords suggesting subagent
	const subagentKeywords = [
		"review",
		"audit",
		"analyze",
		"check",
		"helper",
		"assist",
		"search",
		"find",
		"explore",
		"scan",
		"inspect",
		"verify",
		"validate",
		"lint",
		"format",
		"test",
		"debug",
		"document",
		"explain",
	]

	// Keywords suggesting primary
	const primaryKeywords = [
		"build",
		"implement",
		"create",
		"develop",
		"main",
		"primary",
		"default",
		"general",
		"full",
		"orchestrat",
	]

	for (const keyword of primaryKeywords) {
		if (text.includes(keyword)) return "primary"
	}

	for (const keyword of subagentKeywords) {
		if (text.includes(keyword)) return "subagent"
	}

	return "primary"
}

/**
 * Infer temperature based on agent purpose.
 */
function inferTemperature(name: string, description?: string): number {
	const text = `${name} ${description ?? ""}`.toLowerCase()

	// Low temperature (precise, analytical)
	if (/security|audit|review|lint|check|verify|validate|test/.test(text)) {
		return 0.1
	}

	// Medium-low (coding tasks)
	if (/code|implement|build|develop|engineer|refactor|fix|debug/.test(text)) {
		return 0.3
	}

	// Medium-high (creative tasks)
	if (/document|write|explain|create|design|architect|plan/.test(text)) {
		return 0.5
	}

	return 0.3
}

function cleanUndefined(obj: object): Record<string, unknown> {
	const result: Record<string, unknown> = {}
	for (const [key, value] of Object.entries(obj)) {
		if (value !== undefined) {
			result[key] = value
		}
	}
	return result
}
