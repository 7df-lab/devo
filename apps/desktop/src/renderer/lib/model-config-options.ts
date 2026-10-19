import { queryKeys, type ModelRef } from "../hooks/use-devo-data"
import { getBaseClient, getProjectClient } from "../services/connection-manager"
import { queryClient } from "./query-client"

export type RuntimeModelConfigID = "model" | "thought_level"

export async function persistRuntimeModelConfigOption(
	directory: string,
	configID: RuntimeModelConfigID,
	value: string,
): Promise<void> {
	const client = directory ? getProjectClient(directory) : getBaseClient()
	if (!client) throw new Error(`No client for directory ${directory}`)
	await client.model.preferences.write({
		patch: configID === "model" ? { model: value } : { reasoningEffort: value },
	})
	await Promise.all([
		queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) }),
		queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) }),
		queryClient.invalidateQueries({ queryKey: ["modelDefaults"] }),
	])
}

export async function persistRuntimeModelSelection(
	directory: string,
	model: ModelRef,
): Promise<void> {
	await persistRuntimeModelConfigOption(
		directory,
		"model",
		model.providerID === "session"
			? model.modelID
			: `${model.providerID}/${model.modelID}`,
	)
}
