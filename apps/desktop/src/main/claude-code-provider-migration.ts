import {
	extractClaudeCodeProviderSettings,
	formatClaudeCodeProviderSettingsPreview,
} from "@devo/configconv"
import type { ClaudeCodeProviderSettings, ClaudeSettings } from "@devo/configconv"

interface MigrationFilePreview {
	path: string
	status: "new" | "modified" | "skipped"
	lineCount: number
	content?: string
}

interface MigrationCategoryPreview {
	category: string
	itemCount: number
	files: MigrationFilePreview[]
}

interface ClaudeCodeProviderMigrationResult {
	category: MigrationCategoryPreview | null
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

interface ClaudeCodeProviderMigrationWriteResult {
	filesWritten: string[]
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

type RequestProviderUpsert = (method: string, params?: unknown) => Promise<unknown>

export function buildClaudeCodeProviderMigrationPreview(
	scanResult: unknown,
): ClaudeCodeProviderMigrationResult {
	const settings = extractClaudeCodeProviderSettings(readClaudeCodeSettings(scanResult))
	const diagnostics = diagnosticsFor(settings)
	if (settings.models.length === 0) {
		return {
			category: null,
			...diagnostics,
		}
	}

	const content = formatClaudeCodeProviderSettingsPreview(settings)
	return {
		category: {
			category: "config",
			itemCount: settings.models.length,
			files: [
				{
					path: `provider/upsert:${settings.providerId}`,
					status: "new",
					lineCount: content.split("\n").length,
					content,
				},
			],
		},
		...diagnostics,
	}
}

export async function executeClaudeCodeProviderMigration(
	scanResult: unknown,
	requestProviderUpsert: RequestProviderUpsert,
): Promise<ClaudeCodeProviderMigrationWriteResult> {
	const settings = extractClaudeCodeProviderSettings(readClaudeCodeSettings(scanResult))
	const diagnostics = diagnosticsFor(settings)
	const filesWritten: string[] = []
	const errors: string[] = [...diagnostics.errors]

	for (const params of buildProviderUpsertParams(settings)) {
		const bindingId = params.model_binding.binding_id
		try {
			await requestProviderUpsert("provider/upsert", params)
			filesWritten.push(`provider/upsert:${settings.providerId}/${bindingId}`)
		} catch (error) {
			errors.push(
				`Claude Code provider migration failed for ${params.model_binding.request_model}: ${error instanceof Error ? error.message : String(error)}`,
			)
		}
	}

	return {
		filesWritten,
		warnings: diagnostics.warnings,
		manualActions: diagnostics.manualActions,
		errors,
	}
}

function buildProviderUpsertParams(settings: ClaudeCodeProviderSettings): Array<{
	provider_vendor: {
		name: string
		base_url: string | null
		credential: null
		headers: null
		wire_apis: string[]
		enabled: true
	}
	model_binding: {
		binding_id: string
		model_slug: string
		provider: string
		request_model: string
		display_name: string
		invocation_method: string
		default_reasoning_effort: null
		enabled: true
	}
	default_model_binding?: string
	api_key?: string
}> {
	return settings.models.map((model) => {
		const bindingId = `${slugComponent(model)}-${settings.providerId}`
		const params = {
			provider_vendor: {
				name: settings.providerId,
				base_url: settings.baseUrl ?? null,
				credential: null,
				headers: null,
				wire_apis: [settings.wireApi],
				enabled: true as const,
			},
			model_binding: {
				binding_id: bindingId,
				model_slug: model,
				provider: settings.providerId,
				request_model: model,
				display_name: model,
				invocation_method: settings.wireApi,
				default_reasoning_effort: null,
				enabled: true as const,
			},
			default_model_binding: model === settings.defaultModel ? bindingId : undefined,
			api_key: settings.apiKey,
		}
		return params
	})
}

function diagnosticsFor(settings: ClaudeCodeProviderSettings): {
	warnings: string[]
	manualActions: string[]
	errors: string[]
} {
	const warnings: string[] = []
	const manualActions: string[] = []

	if (settings.models.length === 0) {
		warnings.push(
			"Claude Code settings did not include ANTHROPIC_MODEL or default Anthropic model env vars; no provider model bindings were imported.",
		)
	}
	if (!settings.apiKey) {
		manualActions.push(
			"Claude Code settings did not include ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY. Add an API key manually after migration.",
		)
	}

	return { warnings, manualActions, errors: [] }
}

function readClaudeCodeSettings(scanResult: unknown): ClaudeSettings | undefined {
	if (!isRecord(scanResult)) return undefined
	const data = scanResult.data
	if (!isRecord(data)) return undefined
	const global = data.global
	if (!isRecord(global)) return undefined
	return isRecord(global.settings) ? (global.settings as ClaudeSettings) : undefined
}

function slugComponent(value: string): string {
	let out = ""
	for (const ch of value) {
		if (/[a-zA-Z0-9]/.test(ch)) {
			out += ch.toLowerCase()
		} else if (!out.endsWith("-")) {
			out += "-"
		}
	}
	const slug = out.replace(/^-+|-+$/g, "")
	return slug || "model"
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value)
}
