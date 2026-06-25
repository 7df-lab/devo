/**
 * Global settings converter.
 *
 * Converts ~/.Claude/settings.json -> ~/.config/devo/devo.json
 */
import type { ClaudeSettings, ClaudeUserState } from "../types/claude-code"
import type { DevoConfig } from "../types/devo"
import type { MigrationReport } from "../types/report"
import { createEmptyReport } from "../types/report"
import { detectProvider, suggestSmallModel, translateModelId } from "./model-id"
import { convertPermissions } from "./permissions"

export interface ConfigConversionInput {
	settings?: ClaudeSettings
	userState?: ClaudeUserState
	modelOverrides?: Record<string, string>
	defaultModel?: string
	defaultSmallModel?: string
}

export interface ConfigConversionResult {
	config: Partial<DevoConfig>
	report: MigrationReport
}

/**
 * Convert Claude Code global settings to Devo config.
 */
export function convertConfig(input: ConfigConversionInput): ConfigConversionResult {
	const { settings, userState, modelOverrides, defaultModel, defaultSmallModel } = input
	const config: Partial<DevoConfig> = {
		$schema: "https://devo.ai/config.json",
	}
	const report = createEmptyReport()

	if (!settings && !userState) {
		report.skipped.push({
			category: "config",
			source: "No Claude Code settings found",
			target: "",
		})
		return { config, report }
	}

	// Model
	if (settings?.model) {
		const provider = detectProvider(settings.env, settings.model)
		const model = translateModelId(settings.model, provider, modelOverrides)
		config.model = model

		report.migrated.push({
			category: "config",
			source: `model: "${settings.model}"`,
			target: `model: "${model}"`,
		})

		// Suggest small model
		config.small_model = defaultSmallModel ?? suggestSmallModel(model)
	} else if (defaultModel) {
		config.model = defaultModel
	}

	// Provider configuration
	if (settings?.env) {
		const providerConfig = buildProviderConfig(settings.env, report)
		if (Object.keys(providerConfig).length > 0) {
			config.provider = providerConfig
		}
	}

	// Auto-updates
	if (settings?.autoUpdatesChannel) {
		config.autoupdate = true
		report.migrated.push({
			category: "config",
			source: `autoUpdatesChannel: "${settings.autoUpdatesChannel}"`,
			target: "autoupdate: true",
		})
	}

	// Permissions
	if (settings?.permissions) {
		const { permission, report: permReport } = convertPermissions(settings.permissions)
		config.permission = permission
		report.migrated.push(...permReport.migrated)
		report.warnings.push(...permReport.warnings)
	}

	// Teammate mode -- no direct Devo equivalent
	if (settings?.teammateMode) {
		report.manualActions.push(
			`Claude Code teammateMode="${settings.teammateMode}" detected. ` +
				`Devo does not have a direct equivalent. ` +
				`Consider using agent configurations or multi-agent workflows instead.`,
		)
	}

	// Environment variables that don't map directly
	if (settings?.env) {
		const unmappedEnvVars = Object.keys(settings.env).filter(
			(key) =>
				!key.startsWith("CLAUDE_CODE_USE_") &&
				!key.startsWith("ANTHROPIC_") &&
				!key.startsWith("AWS_") &&
				!key.startsWith("GOOGLE_"),
		)
		if (unmappedEnvVars.length > 0) {
			report.manualActions.push(
				`The following environment variables from Claude Code settings have no direct Devo equivalent: ${unmappedEnvVars.join(", ")}. ` +
					`You may need to set these in your shell profile or use {env:VAR} interpolation in devo.json.`,
			)
		}
	}

	// Hooks -> manual action
	if (settings?.hooks) {
		const hookTypes = Object.keys(settings.hooks).filter((k) => {
			const entries = (settings.hooks as Record<string, unknown[]>)[k]
			return Array.isArray(entries) && entries.length > 0
		})
		if (hookTypes.length > 0) {
			report.manualActions.push(
				`Claude Code hooks detected (${hookTypes.join(", ")}). ` +
					`Devo uses a plugin system instead of hooks. ` +
					`Consider creating a plugin in .devo/plugins/ -- see hooks converter output.`,
			)
		}
	}

	return { config, report }
}

function buildProviderConfig(
	env: Record<string, string>,
	report: MigrationReport,
): Record<string, { options?: Record<string, unknown> }> {
	const providers: Record<string, { options?: Record<string, unknown> }> = {}

	if (env.CLAUDE_CODE_USE_BEDROCK === "1") {
		providers["amazon-bedrock"] = { options: {} }
		report.migrated.push({
			category: "config",
			source: "CLAUDE_CODE_USE_BEDROCK=1",
			target: 'provider: "amazon-bedrock"',
		})

		// Don't copy AWS credentials -- use env var references
		if (env.AWS_ACCESS_KEY_ID || env.AWS_SECRET_ACCESS_KEY) {
			report.manualActions.push(
				"AWS credentials detected in Claude Code env. Devo reads AWS credentials " +
					"from environment variables or ~/.aws/credentials automatically. " +
					"Do NOT put credentials in devo.json.",
			)
		}
	}

	if (env.CLAUDE_CODE_USE_VERTEX === "1") {
		providers["google-vertex"] = { options: {} }
		report.migrated.push({
			category: "config",
			source: "CLAUDE_CODE_USE_VERTEX=1",
			target: 'provider: "google-vertex"',
		})
	}

	if (env.ANTHROPIC_API_KEY) {
		// Don't copy the actual key
		providers.anthropic = {
			options: { apiKey: "{env:ANTHROPIC_API_KEY}" },
		}
		report.warnings.push(
			"Anthropic API key detected. Using {env:ANTHROPIC_API_KEY} reference " +
				"instead of copying the secret. Ensure ANTHROPIC_API_KEY is set in your environment.",
		)
	}

	return providers
}
