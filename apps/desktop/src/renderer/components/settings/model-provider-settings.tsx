import { useQuery } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { useState } from "react"
import { lastProjectDirectoryAtom } from "../../atoms/preferences"
import { serverConnectedAtom } from "../../atoms/connection"
import {
	getModelCurrentVariant,
	getModelVariants,
	modelAllowsDefaultVariant,
	resolveEffectiveModel,
	type ModelRef,
} from "../../hooks/use-devo-data"
import {
	persistRuntimeModelConfigOption,
	persistRuntimeModelSelection,
} from "../../lib/model-config-options"
import {
	getBaseClient,
	getProjectClient,
} from "../../services/connection-manager"
import { ModelSelector } from "../chat/model-selector"
import { ModelSettings } from "./model-settings"
import { ProviderSettings } from "./provider-settings"
import { SettingsHeader } from "./settings-header"
import { settingsPageClass } from "./settings-surface"

/** Settings and the composer use the same catalog, connection flows and selection. */
export function ModelProviderSettings() {
	const directory = useAtomValue(lastProjectDirectoryAtom) ?? ""
	const connected = useAtomValue(serverConnectedAtom)
	const [section, setSection] = useState<"browse" | "connections" | "advanced">(
		"browse",
	)
	const [status, setStatus] = useState<string | null>(null)
	const [error, setError] = useState<string | null>(null)
	const defaults = useQuery({
		queryKey: ["modelDefaults", directory],
		enabled: connected,
		queryFn: async () => {
			const client = directory ? getProjectClient(directory) : getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const [providers, config] = await Promise.all([
				client.config.providers(),
				client.config.get(),
			])
			return {
				providers: {
					providers: providers.data.providers,
					defaults: providers.data.default,
				},
				model: config.data.model,
			}
		},
	})
	const providers = defaults.data?.providers ?? null
	const model = resolveEffectiveModel(
		null,
		null,
		defaults.data?.model,
		providers?.defaults ?? {},
		providers?.providers ?? [],
	)
	const variants = model
		? getModelVariants(
				model.providerID,
				model.modelID,
				providers?.providers ?? [],
			)
		: []
	const currentVariant = model
		? getModelCurrentVariant(
				model.providerID,
				model.modelID,
				providers?.providers ?? [],
			)
		: undefined
	const save = async (operation: Promise<void>) => {
		setStatus("Saving…")
		setError(null)
		try {
			await operation
			setStatus("Saved for new chats")
		} catch (error) {
			setStatus(null)
			setError(
				error instanceof Error
					? error.message
					: "Failed to save model preference",
			)
		}
	}
	return (
		<div className={settingsPageClass}>
			<SettingsHeader
				title="Models & providers"
				description="Find a model, connect a provider, and choose your defaults."
			/>
			<div
				className="flex gap-1 border-b pb-2"
				role="group"
				aria-label="Model settings sections"
			>
				{(["browse", "connections", "advanced"] as const).map((value) => (
					<button
						key={value}
						type="button"
						aria-pressed={section === value}
						onClick={() => setSection(value)}
						className={`rounded-md px-3 py-1.5 text-sm ${section === value ? "bg-muted text-foreground" : "text-muted-foreground hover:bg-muted/60"}`}
					>
						{value === "browse"
							? "Models"
							: value === "connections"
								? "Connections"
								: "Model configuration"}
					</button>
				))}
			</div>
			{section === "browse" ? (
				<>
					<p className="text-xs text-muted-foreground">
						{directory
							? `New-chat defaults for ${directory.split(/[\\/]/).filter(Boolean).at(-1)}`
							: "New-chat defaults"}
						. Existing chats keep their own selections.
					</p>
					<ModelSelector
						presentation="inline"
						providers={providers}
						effectiveModel={model}
						hasOverride={false}
						variants={variants}
						currentVariant={currentVariant}
						allowDefaultVariant={
							model
								? modelAllowsDefaultVariant(
										model.providerID,
										model.modelID,
										providers?.providers ?? [],
									)
								: true
						}
						onSelectModel={(model: ModelRef | null) => {
							if (model)
								void save(persistRuntimeModelSelection(directory, model))
						}}
						onSelectVariant={(variant) => {
							if (variant)
								void save(
									persistRuntimeModelConfigOption(
										directory,
										"thought_level",
										variant,
									),
								)
						}}
						onManageProviders={() => setSection("connections")}
					/>
					{status && (
						<p role="status" className="text-xs text-muted-foreground">
							{status}
						</p>
					)}
					{(error || defaults.error) && (
						<p role="alert" className="text-sm text-destructive">
							{error ?? defaults.error?.message}
						</p>
					)}
				</>
			) : section === "connections" ? (
				<ProviderSettings />
			) : (
				<ModelSettings />
			)}
		</div>
	)
}
