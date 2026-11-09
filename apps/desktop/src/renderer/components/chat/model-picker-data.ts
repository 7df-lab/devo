import type { ModelRef, ProvidersData, ProviderCatalogData } from "../../hooks/use-devo-data"
import type { ModelDirectoryData } from "../../hooks/use-model-directory"

export interface PickerModel {
	value: string
	selection: ModelRef
	providerID: string
	providerName: string
	modelID: string
	displayName: string
	ready: boolean
}

/** Native preference values are authoritative, including connection and variant paths. */
export function buildPickerModels(
	available: ProvidersData | null,
	catalog: ProviderCatalogData | null,
	directory: ModelDirectoryData | null = null,
): PickerModel[] {
	const models = new Map<string, PickerModel>()
	const configured = new Set([
		...(catalog?.connectedIds ?? []),
		...(directory?.credentials.map((credential) => credential.provider) ?? []),
	])
	for (const provider of available?.providers ?? []) {
		for (const [modelID, model] of Object.entries(provider.models)) {
			const canonical = provider.id === "session" ? modelID : `${provider.id}/${modelID}`
			const providerID =
				provider.id === "session" && modelID.includes("/") ? modelID.split("/")[0] : provider.id
			const providerInfo = catalog?.providers.find((entry) => entry.id === providerID)
			const localID =
				provider.id === "session" && modelID.includes("/")
					? modelID.slice(providerID.length + 1)
					: modelID
			models.set(canonical, {
				value: canonical,
				selection: { providerID: provider.id, modelID },
				providerID,
				providerName: providerInfo?.name ?? provider.name,
				modelID: localID,
				displayName:
					(model as { name?: string }).name === modelID
						? providerInfo?.models?.[localID]?.name || modelID
						: (model as { name?: string }).name || modelID,
				ready: true,
			})
		}
	}
	for (const model of directory?.models ?? []) {
		const providerID =
			model.providerId ?? (model.slug.includes("/") ? model.slug.split("/")[0] : "session")
		const provider = catalog?.providers.find((entry) => entry.id === providerID)
		if (model.enabled === false || provider?.enabled === false || models.has(model.slug)) continue
		models.set(model.slug, {
			value: model.slug,
			selection: { providerID: "session", modelID: model.slug },
			providerID,
			providerName: provider?.name ?? (providerID === "session" ? "Configured model" : providerID),
			modelID: model.modelId ?? model.slug.slice(model.slug.indexOf("/") + 1),
			displayName: model.displayName,
			ready: providerID === "session" || configured.has(providerID),
		})
	}
	for (const provider of catalog?.providers ?? []) {
		if (!provider.enabled) continue
		for (const [modelID, model] of Object.entries(provider.models ?? {})) {
			if (model.enabled === false) continue
			const value = `${provider.id}/${modelID}`
			if (models.has(value)) continue
			models.set(value, {
				value,
				selection: { providerID: "session", modelID: value },
				providerID: provider.id,
				providerName: provider.name,
				modelID,
				displayName: model.name || modelID,
				ready: configured.has(provider.id),
			})
		}
	}
	return [...models.values()].sort(
		(left, right) =>
			Number(right.ready) - Number(left.ready) ||
			left.providerName.localeCompare(right.providerName) ||
			left.displayName.localeCompare(right.displayName) ||
			left.value.localeCompare(right.value),
	)
}

export function filterPickerModels(
	models: PickerModel[],
	search: string,
	scope: "ready" | "all",
): PickerModel[] {
	const terms = search.toLowerCase().trim().split(/\s+/).filter(Boolean)
	return models.filter(
		(model) =>
			(scope === "all" || model.ready) &&
			terms.every((term) =>
				`${model.displayName} ${model.modelID} ${model.providerName} ${model.providerID}`
					.toLowerCase()
					.includes(term),
			),
	)
}
