/**
 * Model ID translation between Claude Code and Devo formats.
 *
 * Claude Code: "claude-opus-4-6" or "anthropic.claude-opus-4-6-v1:0:1m" (Bedrock ARN)
 * Devo: "provider/model" format (e.g., "anthropic/claude-opus-4-6")
 */

/** Known model name -> Devo provider/model mappings */
const MODEL_MAP: Record<string, string> = {
	// Anthropic direct models
	"claude-opus-4-6": "anthropic/claude-opus-4-6",
	"claude-opus-4-5": "anthropic/claude-opus-4-5",
	"claude-opus-4-5-20250410": "anthropic/claude-opus-4-5",
	"claude-sonnet-4-5": "anthropic/claude-sonnet-4-5",
	"claude-sonnet-4-5-20250514": "anthropic/claude-sonnet-4-5",
	"claude-sonnet-4-5-20250929": "anthropic/claude-sonnet-4-5",
	"claude-sonnet-4": "anthropic/claude-sonnet-4",
	"claude-3-5-sonnet-20241022": "anthropic/claude-3-5-sonnet-20241022",
	"claude-3-5-haiku-20241022": "anthropic/claude-3-5-haiku-20241022",
	"claude-3-opus-20240229": "anthropic/claude-3-opus-20240229",
	"claude-3-haiku-20240307": "anthropic/claude-3-haiku-20240307",

	// Short aliases used in Claude Code agent definitions
	opus: "anthropic/claude-opus-4-6",
	sonnet: "anthropic/claude-sonnet-4-5",
	haiku: "anthropic/claude-3-5-haiku-20241022",
}

/**
 * Detect the provider from environment variables or model ID patterns.
 */
export function detectProvider(
	env?: Record<string, string>,
	modelId?: string,
): "anthropic" | "amazon-bedrock" | "google-vertex" {
	if (env?.CLAUDE_CODE_USE_BEDROCK === "1") return "amazon-bedrock"
	if (env?.CLAUDE_CODE_USE_VERTEX === "1") return "google-vertex"
	if (modelId) {
		if (
			modelId.startsWith("anthropic.") ||
			modelId.startsWith("us.anthropic.") ||
			modelId.startsWith("eu.anthropic.") ||
			modelId.startsWith("ap.anthropic.") ||
			modelId.startsWith("global.anthropic.")
		) {
			return "amazon-bedrock"
		}
	}
	return "anthropic"
}

/**
 * Translate a Claude Code model ID to Devo format.
 *
 * @param ccModelId - Claude Code model identifier
 * @param provider - Detected or overridden provider
 * @param overrides - Manual model ID overrides
 * @returns Devo "provider/model" string
 */
export function translateModelId(
	ccModelId: string,
	provider?: "anthropic" | "amazon-bedrock" | "google-vertex",
	overrides?: Record<string, string>,
): string {
	// Check manual overrides first
	if (overrides?.[ccModelId]) {
		return overrides[ccModelId]
	}

	// Already in provider/model format
	if (ccModelId.includes("/")) {
		return ccModelId
	}

	// Direct mapping from known models
	if (MODEL_MAP[ccModelId]) {
		return MODEL_MAP[ccModelId]
	}

	// Bedrock ARN-style model IDs
	if (
		ccModelId.startsWith("anthropic.") ||
		ccModelId.startsWith("us.anthropic.") ||
		ccModelId.startsWith("eu.anthropic.") ||
		ccModelId.startsWith("ap.anthropic.") ||
		ccModelId.startsWith("global.anthropic.")
	) {
		return `amazon-bedrock/${ccModelId}`
	}

	// Starts with claude- prefix -> Anthropic
	if (ccModelId.startsWith("claude-")) {
		return `anthropic/${ccModelId}`
	}

	// Fallback: use detected provider
	const resolvedProvider = provider ?? "anthropic"
	return `${resolvedProvider}/${ccModelId}`
}

/**
 * Suggest a small model based on the main model's provider.
 */
export function suggestSmallModel(mainModel: string): string {
	if (mainModel.startsWith("amazon-bedrock/")) {
		return "amazon-bedrock/anthropic.claude-sonnet-4-5-20250929-v1:0"
	}
	if (mainModel.startsWith("google-vertex/")) {
		return "google-vertex/claude-sonnet-4-5"
	}
	return "anthropic/claude-sonnet-4-5"
}

/**
 * Check if a model ID looks valid (has provider/ prefix).
 */
export function isValidModelId(modelId: string): boolean {
	return modelId.includes("/") && modelId.split("/").length >= 2
}
