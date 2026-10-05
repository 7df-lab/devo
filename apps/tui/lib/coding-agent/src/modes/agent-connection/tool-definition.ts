import type { ToolDefinition } from "../../core/extensions/index.js";
import type { AgentConnectionToolDefinition } from "./types.js";

export function createAgentConnectionToolDefinition(
	definition: ToolDefinition | undefined,
): AgentConnectionToolDefinition | undefined {
	if (!definition) {
		return undefined;
	}

	return {
		name: definition.name,
		label: definition.label,
		description: definition.description,
		...(definition.promptSnippet !== undefined ? { promptSnippet: definition.promptSnippet } : {}),
		...(definition.promptGuidelines !== undefined ? { promptGuidelines: [...definition.promptGuidelines] } : {}),
		parameters: definition.parameters,
		...(definition.renderShell !== undefined ? { renderShell: definition.renderShell } : {}),
		...(definition.replayBuiltInToolName !== undefined
			? { replayBuiltInToolName: definition.replayBuiltInToolName }
			: {}),
	};
}
