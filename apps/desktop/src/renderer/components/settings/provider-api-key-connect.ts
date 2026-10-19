/** Persist a provider Connection and its write-only API key through the native SDK. */
import type { DevoClient } from "@devo-ai/sdk/v2/client"
import type { CatalogProvider } from "../../hooks/use-devo-data"

type ProviderConnectionClient = { provider: Pick<DevoClient["provider"], "upsert"> }

export async function connectProviderWithApiKey(
	client: ProviderConnectionClient,
	provider: CatalogProvider,
	apiKey: string,
	configOptions?: Record<string, string>,
): Promise<void> {
	// Use the complete catalog entry so the native Connection keeps its built-in
	// endpoint, wire protocol, model directory, and credential configuration.
	// provider/upsert writes apiKey to the user auth store; it is never put in
	// provider.options or the persisted provider catalog.
	const { env: _env, ...template } = provider
	const options = configOptions
		? {
				...(template.options &&
				typeof template.options === "object" &&
				!Array.isArray(template.options)
					? template.options
					: {}),
				...configOptions,
			}
		: template.options
	await client.provider.upsert({
		provider: { ...template, ...(configOptions ? { options } : {}) },
		apiKey: apiKey.trim(),
	})
}
