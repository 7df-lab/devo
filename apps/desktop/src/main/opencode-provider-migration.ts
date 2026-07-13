import {
	extractOpenCodeProviderSettings,
	formatOpenCodeProviderSettingsPreview,
} from "@devo/configconv"
import type {
	OpenCodeImportedProvider,
	OpenCodeProviderSettings,
	OpenCodeScanResult,
} from "@devo/configconv"

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

interface OpenCodeProviderMigrationResult {
	category: MigrationCategoryPreview | null
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

interface OpenCodeProviderMigrationWriteResult {
	filesWritten: string[]
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

type RequestProviderUpsert = (method: string, params?: unknown) => Promise<unknown>

export function buildOpenCodeProviderMigrationPreview(
	scanResult: unknown,
): OpenCodeProviderMigrationResult {
	const settings = extractOpenCodeProviderSettings(readOpenCodeScanResult(scanResult))
	const diagnostics = diagnosticsFor(settings)
	const providers = settings.providers.filter((provider) => provider.models.length > 0)

	if (providers.length === 0) {
		return {
			category: null,
			...diagnostics,
		}
	}

	const files = providers.map((provider) => {
		const content = formatOpenCodeProviderSettingsPreview({
			...settings,
			providers: [provider],
		})
		return {
			path: `provider/upsert:${provider.providerId}`,
			status: "new" as const,
			lineCount: content.split("\n").length,
			content,
		}
	})

	return {
		category: {
			category: "config",
			itemCount: providers.reduce((sum, provider) => sum + provider.models.length, 0),
			files,
		},
		...diagnostics,
	}
}

export async function executeOpenCodeProviderMigration(
	scanResult: unknown,
	requestProviderUpsert: RequestProviderUpsert,
): Promise<OpenCodeProviderMigrationWriteResult> {
	const settings = extractOpenCodeProviderSettings(readOpenCodeScanResult(scanResult))
	const diagnostics = diagnosticsFor(settings)
	const filesWritten: string[] = []
	const errors: string[] = [...diagnostics.errors]

	for (const params of buildProviderUpsertParams(settings)) {
		const providerName = params.model_binding.provider
		const requestModel = params.model_binding.request_model
		const bindingId = params.model_binding.binding_id
		try {
			await requestProviderUpsert("provider/upsert", params)
			filesWritten.push(`provider/upsert:${providerName}/${bindingId}`)
		} catch (error) {
			errors.push(
				`OpenCode provider migration failed for ${providerName}/${requestModel}: ${error instanceof Error ? error.message : String(error)}`,
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

function buildProviderUpsertParams(settings: OpenCodeProviderSettings): Array<{
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
	const params = []

	for (const provider of settings.providers) {
		for (const model of provider.models) {
			const bindingId = `${slugComponent(model.modelId)}-${slugComponent(provider.providerId)}`
			params.push({
				provider_vendor: {
					name: provider.providerId,
					base_url: provider.baseUrl ?? null,
					credential: null,
					headers: null,
					wire_apis: [provider.wireApi],
					enabled: true as const,
				},
				model_binding: {
					binding_id: bindingId,
					model_slug: model.modelId,
					provider: provider.providerId,
					request_model: model.modelId,
					display_name: model.displayName,
					invocation_method: provider.wireApi,
					default_reasoning_effort: null,
					enabled: true as const,
				},
				default_model_binding: model.isDefault ? bindingId : undefined,
				api_key: provider.apiKey,
			})
		}
	}

	return params
}

function diagnosticsFor(settings: OpenCodeProviderSettings): {
	warnings: string[]
	manualActions: string[]
	errors: string[]
} {
	const warnings: string[] = [...settings.parseErrors]
	const manualActions: string[] = []

	if (!settings.configPath && settings.providers.length === 0 && settings.unsupportedProviders.length === 0) {
		warnings.push(
			"OpenCode config was not found at ~/.config/opencode/opencode.json or ~/.config/opencode/opencode.jsonc.",
		)
	}

	for (const provider of settings.unsupportedProviders) {
		warnings.push(
			`OpenCode provider ${provider.providerId} uses unsupported npm package ${provider.npm ?? "(not set)"}; only @ai-sdk/openai-compatible providers are imported.`,
		)
	}

	for (const provider of settings.providers) {
		pushProviderDiagnostics(provider, warnings, manualActions)
	}

	if (settings.providers.reduce((sum, provider) => sum + provider.models.length, 0) === 0) {
		warnings.push(
			"OpenCode settings did not include any importable OpenAI-compatible provider models; no provider model bindings were imported.",
		)
	}

	return { warnings, manualActions, errors: [] }
}

function pushProviderDiagnostics(
	provider: OpenCodeImportedProvider,
	warnings: string[],
	manualActions: string[],
): void {
	if (!provider.baseUrl) {
		warnings.push(
			`OpenCode provider ${provider.providerId} did not include options.baseURL; migrated provider will need a baseURL before use.`,
		)
	}
	if (provider.models.length === 0) {
		warnings.push(
			`OpenCode provider ${provider.providerId} did not include model definitions; no model bindings were imported for this provider.`,
		)
	}
	if (!provider.apiKey) {
		manualActions.push(
			`OpenCode provider ${provider.providerId} did not include an API key in opencode.json or auth.json. Add an API key manually after migration.`,
		)
	}
}

function readOpenCodeScanResult(scanResult: unknown): OpenCodeScanResult | undefined {
	if (!isRecord(scanResult)) return undefined
	const data = scanResult.data
	if (!isRecord(data)) return undefined
	const global = data.global
	if (!isRecord(global)) return undefined
	return {
		global: {
			config: isRecord(global.config) ? global.config : undefined,
			configPath: typeof global.configPath === "string" ? global.configPath : undefined,
			auth: isRecord(global.auth) ? global.auth : undefined,
			authPath: typeof global.authPath === "string" ? global.authPath : undefined,
			parseErrors: Array.isArray(global.parseErrors)
				? global.parseErrors.filter((item): item is string => typeof item === "string")
				: [],
		},
	}
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
