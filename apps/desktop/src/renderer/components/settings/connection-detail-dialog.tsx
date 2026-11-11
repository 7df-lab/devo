/**
 * Dialog showing a connected provider's model list.
 *
 * Allows adding models, removing manually added models, and triggering
 * model discovery from the provider's /models endpoint.
 */

import type {
	CatalogModelInfo,
	CatalogProviderInfo,
} from "@devo-ai/sdk/v2/client"
import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Spinner } from "@devo/ui/components/spinner"
import { PlusIcon, RefreshCwIcon, SearchIcon, Trash2Icon } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { getBaseClient } from "../../services/connection-manager"
import {
	effectiveContextWindowTokens,
	formatContextWindowLabel,
} from "../../lib/providers"
import { ModelEditDialog } from "./model-edit-dialog"
import { ProviderIcon } from "./provider-icon"

interface ConnectionDetailDialogProps {
	provider: CatalogProviderInfo
	connectionModels: Record<string, CatalogModelInfo>
	open: boolean
	onOpenChange: (open: boolean) => void
	onChanged: () => void
	onEdit?: (provider: CatalogProviderInfo) => void
}

export function ConnectionDetailDialog({
	provider,
	connectionModels,
	open,
	onOpenChange,
	onChanged,
	onEdit,
}: ConnectionDetailDialogProps) {
	const [search, setSearch] = useState("")
	const [editing, setEditing] = useState(false)
	const [discovering, setDiscovering] = useState(false)
	const [removingModel, setRemovingModel] = useState<string | null>(null)
	const [addingModel, setAddingModel] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [discovered, setDiscovered] = useState<Record<string, CatalogModelInfo>>({})
	const [savingModel, setSavingModel] = useState<string | null>(null)
	const discoveryVersion = useRef(0)
	const modelMutationPending = removingModel !== null || savingModel !== null || addingModel
	const mutationPending = modelMutationPending || editing
	useEffect(() => {
		discoveryVersion.current++
		setDiscovering(false)
		setDiscovered({})
		setSearch("")
		setError(null)
		return () => { discoveryVersion.current++ }
	}, [provider.id, open])

	const models = useMemo(() => {
		const entries = Object.entries({ ...provider.models, ...discovered, ...connectionModels })
		if (!search.trim()) return entries
		const lower = search.toLowerCase()
		return entries.filter(
			([id, m]) =>
				id.toLowerCase().includes(lower) ||
				(m.name ?? "").toLowerCase().includes(lower),
		)
	}, [provider.models, discovered, connectionModels, search])

	const handleDiscover = useCallback(async () => {
		const version = ++discoveryVersion.current
		setDiscovering(true)
		setError(null)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const result = await client.provider.discover({
				providerId: provider.id,
				forceRefresh: true,
			})
			if (version === discoveryVersion.current) setDiscovered(result.models)
		} catch (error) {
			if (version !== discoveryVersion.current) return
			setError(
				error instanceof Error ? error.message : "Failed to discover models",
			)
		} finally {
			if (version === discoveryVersion.current) setDiscovering(false)
		}
	}, [provider.id])

	const handleSaveModel = useCallback(async (modelId: string, model: CatalogModelInfo) => {
		setSavingModel(modelId)
		setError(null)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			await client.provider.upsert({
				provider: { ...provider, models: { [modelId]: {
					origin: "user",
					baseUrl: model.baseUrl,
					wireApi: model.wireApi,
					requestModel: model.requestModel,
				} } },
			})
			onChanged()
		} catch (error) {
			setError(error instanceof Error ? error.message : "Failed to save model")
		} finally {
			setSavingModel(null)
		}
	}, [provider, onChanged])

	const handleRemoveModel = useCallback(
		async (modelId: string) => {
			setRemovingModel(modelId)
			setError(null)
			try {
				const client = getBaseClient()
				if (!client) throw new Error("Not connected to server")
				await client.provider.modelRemove({
					providerId: provider.id,
					modelId,
				})
				onChanged()
			} catch (error) {
				setError(
					error instanceof Error ? error.message : "Failed to remove model",
				)
			} finally {
				setRemovingModel(null)
			}
		},
		[provider.id, onChanged],
	)

	const handleEdit = useCallback(async () => {
		if (!onEdit || mutationPending) return
		const version = ++discoveryVersion.current
		// Editing supersedes discovery, including its pending indicator.
		setDiscovering(false)
		setEditing(true)
		setError(null)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			// Read after pending mutations finish, rather than copying a stale catalog snapshot.
			const { data } = await client.provider.list()
			if (version !== discoveryVersion.current) return
			const current = data.providers.find((entry: CatalogProviderInfo) => entry.id === provider.id)
			if (!current || !data.connectedProviderIds.includes(provider.id)) {
				throw new Error("This provider is no longer connected")
			}
			onEdit({ ...current, models: data.connectionModels[provider.id] ?? {} })
		} catch (error) {
			if (version === discoveryVersion.current) {
				setError(error instanceof Error ? error.message : "Failed to load connection")
			}
		} finally {
			if (version === discoveryVersion.current) setEditing(false)
		}
	}, [mutationPending, onEdit, provider.id])

	return (
		<>
			<Dialog open={open} onOpenChange={(nextOpen) => {
				if (modelMutationPending) return
				if (!nextOpen) discoveryVersion.current++
				onOpenChange(nextOpen)
			}}>
				<DialogContent className="max-w-lg max-h-[80vh] overflow-y-auto">
					<DialogHeader>
						<div className="flex items-center gap-3">
							<ProviderIcon id={provider.id} name={provider.name} size="xs" />
							<DialogTitle>{provider.name}</DialogTitle>
							<Badge variant="secondary">Configured</Badge>
						</div>
						<DialogDescription>
							{Object.keys({ ...provider.models, ...discovered, ...connectionModels }).length}{" "}
							model
							{Object.keys({ ...provider.models, ...discovered, ...connectionModels })
								.length !== 1
								? "s"
								: ""}{" "}
							listed.
							{provider.baseUrl ? ` Endpoint: ${provider.baseUrl}` : ""}
							{" Model access has not been verified by saving this connection. Discovery is temporary. Save a model to keep it in your configuration."}
						</DialogDescription>
					</DialogHeader>

					{/* Toolbar */}
					<div className="flex items-center gap-2">
						<div className="relative flex-1">
							<SearchIcon className="absolute left-3 top-1/2 size-3.5 -translate-y-1/2 stroke-[1.5] text-muted-foreground" />
							<Input
								placeholder="Filter models…"
								value={search}
								onChange={(e) => setSearch(e.target.value)}
								className="h-8 pl-8 text-[13px]"
							/>
						</div>
						<Button
							variant="outline"
							size="sm"
							onClick={handleDiscover}
							disabled={discovering || mutationPending}
						>
							{discovering ? (
								<Spinner className="size-3.5" />
							) : (
								<RefreshCwIcon className="size-3.5 stroke-[1.5]" />
							)}
							Discover
						</Button>
						<Button
							variant="outline"
							size="sm"
							onClick={() => setAddingModel(true)}
							disabled={mutationPending}
						>
							<PlusIcon className="size-3.5 stroke-[1.5]" />
							Add
						</Button>
					</div>
					{error && (
						<p role="alert" className="text-sm text-destructive">
							{error}
						</p>
					)}

					{/* Model list */}
					<div className="divide-y divide-border/40 overflow-hidden rounded-xl border border-border/50">
						{models.length === 0 ? (
							<div className="px-4 py-6 text-center text-xs text-muted-foreground">
								{search.trim()
									? `No models match "${search}"`
									: "No models listed. Discover or add a model."}
							</div>
						) : (
							models.map(([modelId, model]) => {
								const contextLabel = formatContextWindowLabel(
									effectiveContextWindowTokens(model),
								)
								return (
									<div
										key={modelId}
										className="flex items-center gap-3 px-4 py-2.5"
									>
										<div className="min-w-0 flex-1">
											<p className="truncate text-sm tracking-tight">
												{model.name ?? modelId}
											</p>
											<p className="truncate text-xs text-muted-foreground">
												{provider.id}/{modelId}
											</p>
										</div>
										{contextLabel && (
											<span className="shrink-0 text-[11px] text-muted-foreground">
												{contextLabel}
											</span>
										)}
										{discovered[modelId] && connectionModels[modelId]?.origin !== "user" && (
											<Button variant="outline" size="sm"
												aria-label={`Save discovered model ${model.name ?? modelId}`}
												onClick={() => handleSaveModel(modelId, model)}
												disabled={mutationPending}>
												Save
											</Button>
										)}
										{connectionModels[modelId]?.origin === "user" &&
									provider.models?.[modelId]?.origin !== "remote" && (
												<Button
													variant="ghost"
													size="sm"
													className="text-muted-foreground hover:text-destructive"
													aria-label={`Remove saved model ${model.name ?? modelId}`}
													onClick={() => handleRemoveModel(modelId)}
													disabled={mutationPending}
												>
													{removingModel === modelId ? (
														<Spinner className="size-3" />
													) : (
														<Trash2Icon className="size-3.5 stroke-[1.5]" />
													)}
												</Button>
											)}
									</div>
								)
							})
						)}
					</div>

					<DialogFooter>
						{onEdit && <Button variant="outline" onClick={handleEdit} disabled={mutationPending}>Edit connection</Button>}
						<Button variant="outline" disabled={modelMutationPending} onClick={() => {
							discoveryVersion.current++
							onOpenChange(false)
						}}>
							Done
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			{addingModel && (
				<ModelEditDialog
					key={`add-${provider.id}`}
					provider={provider}
					mode="create"
					open={addingModel}
					onOpenChange={(open) => {
						if (!open) setAddingModel(false)
					}}
					onSaved={onChanged}
				/>
			)}
		</>
	)
}
