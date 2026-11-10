/**
 * Create or edit a provider Connection. Add requires an unused connection ID;
 * reusing a built-in template ID requires explicit overlay confirmation.
 */

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
import { Label } from "@devo/ui/components/label"
import {
	Select,
	SelectContent,
	SelectGroup,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Spinner } from "@devo/ui/components/spinner"
import type { CatalogModelInfo, CatalogProviderInfo, CatalogWireApi } from "@devo-ai/sdk/v2/client"
import { PlusIcon, Trash2Icon } from "lucide-react"
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react"
import { getBaseClient } from "../../services/connection-manager"
import { isBuiltinTemplateProviderId, suggestNonTemplateProviderId } from "./custom-provider-id"
import { isValidProviderBaseUrl } from "./provider-base-url"

const WIRE_API_OPTIONS: Array<{ value: CatalogWireApi; label: string }> = [
	{ value: "openai_chat_completions", label: "OpenAI Chat Completions" },
	{ value: "openai_responses", label: "OpenAI Responses" },
	{ value: "anthropic_messages", label: "Anthropic Messages" },
	{ value: "google_generative_ai", label: "Google Generative AI" },
]

interface CustomProviderDialogProps {
	/** Pass an existing provider to edit; omit for new. */
	provider?: CatalogProviderInfo
	/** Built-in template ids from `provider/list` — used to detect collisions. */
	templateIds?: ReadonlySet<string>
	/** Existing Connections cannot be replaced through the Add flow. */
	connectedIds?: ReadonlySet<string>
	open: boolean
	onOpenChange: (open: boolean) => void
	onSaved: () => void
}

interface ModelEntry {
	id: string
	name: string
}

interface HeaderEntry {
	key: string
	value: string
}

export function CustomProviderDialog({
	provider: existing,
	templateIds = new Set(),
	connectedIds = new Set(),
	open,
	onOpenChange,
	onSaved,
}: CustomProviderDialogProps) {
	const isEdit = !!existing

	const [providerId, setProviderId] = useState(existing?.id ?? "")
	const [displayName, setDisplayName] = useState(existing?.name ?? "")
	const [baseUrl, setBaseUrl] = useState(existing?.baseUrl ?? "")
	const [wireApi, setWireApi] = useState<CatalogWireApi>(
		existing?.wireApis[0] ?? "openai_chat_completions",
	)
	const [apiKey, setApiKey] = useState("")
	const [models, setModels] = useState<ModelEntry[]>(() => {
		if (!existing) return [{ id: "", name: "" }]
		return Object.entries(existing.models ?? {}).map(([id, m]) => ({
			id,
			name: m.name ?? "",
		}))
	})
	const [headers, setHeaders] = useState<HeaderEntry[]>(() => {
		if (!existing?.headers) return []
		return Object.entries(existing.headers).map(([key, value]) => ({
			key,
			value,
		}))
	})
	const [saving, setSaving] = useState(false)
	const [error, setError] = useState<{
		message: string
		field?: "provider-id" | "base-url"
	} | null>(null)
	const errorId = useId()
	const providerIdInput = useRef<HTMLInputElement>(null)
	const baseUrlInput = useRef<HTMLInputElement>(null)
	const [builtinConflictOpen, setBuiltinConflictOpen] = useState(false)
	const saveScope = useRef<{ saving: boolean } | null>(null)

	// A completed write still refreshes shared data after dismissal, but its UI
	// result belongs only to the form opening that started it.
	useEffect(() => {
		saveScope.current = open ? { saving: false } : null
		setSaving(false)
		setError(null)
		setBuiltinConflictOpen(false)
		return () => {
			saveScope.current = null
		}
	}, [open])

	const suggestedAlternateId = useMemo(
		() => suggestNonTemplateProviderId(providerId, new Set([...templateIds, ...connectedIds])),
		[providerId, templateIds, connectedIds],
	)

	const addModel = useCallback(() => {
		setModels((prev) => [...prev, { id: "", name: "" }])
	}, [])

	const removeModel = useCallback((index: number) => {
		setModels((prev) => prev.filter((_, i) => i !== index))
	}, [])

	const updateModel = useCallback((index: number, field: "id" | "name", value: string) => {
		setModels((prev) => prev.map((m, i) => (i === index ? { ...m, [field]: value } : m)))
	}, [])

	const addHeader = useCallback(() => {
		setHeaders((prev) => [...prev, { key: "", value: "" }])
	}, [])

	const removeHeader = useCallback((index: number) => {
		setHeaders((prev) => prev.filter((_, i) => i !== index))
	}, [])

	const updateHeader = useCallback((index: number, field: "key" | "value", value: string) => {
		setHeaders((prev) => prev.map((h, i) => (i === index ? { ...h, [field]: value } : h)))
	}, [])

	const persistProvider = useCallback(async () => {
		const scope = saveScope.current
		if (!scope || scope.saving) return
		scope.saving = true
		setSaving(true)
		setError(null)
		try {
			const id = providerId.trim()
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")

			const catalogModels: Record<string, CatalogModelInfo> = {}
			for (const m of models) {
				const mid = m.id.trim()
				if (mid) {
					catalogModels[mid] = { ...existing?.models?.[mid], name: m.name.trim() || undefined }
				}
			}

			const catalogHeaders: Record<string, string> = {}
			for (const h of headers) {
				if (h.key.trim()) {
					catalogHeaders[h.key.trim()] = h.value
				}
			}

			const provider: CatalogProviderInfo = {
				...existing,
				id,
				name: displayName.trim() || id,
				baseUrl: baseUrl.trim(),
				wireApis: [...new Set([wireApi, ...(existing?.wireApis ?? [])])],
				models: catalogModels,
				enabled: existing?.enabled ?? true,
				...(existing || Object.keys(catalogHeaders).length > 0 ? { headers: catalogHeaders } : {}),
			}

			await client.provider.upsert({
				provider,
				...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
			})

			if (saveScope.current === scope) onOpenChange(false)
			onSaved()
		} catch (err) {
			if (saveScope.current === scope) {
				setError({
					message: err instanceof Error ? err.message : "Failed to save provider",
				})
			}
		} finally {
			if (saveScope.current === scope) {
				scope.saving = false
				setSaving(false)
			}
		}
	}, [existing, providerId, displayName, baseUrl, wireApi, apiKey, models, headers, onOpenChange, onSaved])

	const handleSave = useCallback(async () => {
		const id = providerId.trim()
		if (!id) {
			setError({ message: "Provider ID is required", field: "provider-id" })
			providerIdInput.current?.focus()
			return
		}
		if (!isEdit && connectedIds.has(id)) {
			setError({
				message: "Provider ID is already connected. Manage that provider or choose a different ID.",
				field: "provider-id",
			})
			providerIdInput.current?.focus()
			return
		}
		if (!baseUrl.trim()) {
			setError({ message: "Base URL is required", field: "base-url" })
			baseUrlInput.current?.focus()
			return
		}

		if (!isValidProviderBaseUrl(baseUrl)) {
			setError({
				message: "Base URL must be a valid http:// or https:// URL",
				field: "base-url",
			})
			baseUrlInput.current?.focus()
			return
		}

		if (!isEdit && isBuiltinTemplateProviderId(id, templateIds)) {
			setError(null)
			setBuiltinConflictOpen(true)
			return
		}

		await persistProvider()
	}, [providerId, baseUrl, isEdit, templateIds, connectedIds, persistProvider])

	const handleOverlayBuiltin = useCallback(async () => {
		setBuiltinConflictOpen(false)
		await persistProvider()
	}, [persistProvider])

	const handleUseSuggestedId = useCallback(() => {
		setProviderId(suggestedAlternateId)
		setBuiltinConflictOpen(false)
		setError(null)
	}, [suggestedAlternateId])

	return (
		<>
			<Dialog open={open} onOpenChange={onOpenChange}>
				<DialogContent className="sm:max-w-2xl h-[85vh] max-h-[85vh] flex flex-col gap-4 overflow-hidden">
					<DialogHeader>
						<DialogTitle>{isEdit ? "Edit Custom Provider" : "Add Custom Provider"}</DialogTitle>
						<DialogDescription>
							Configure an OpenAI-compatible, Anthropic-compatible, or Google AI provider.
						</DialogDescription>
					</DialogHeader>

					<div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto py-2 pr-1">
						{/* Provider ID */}
						<div className="flex flex-col gap-1.5">
							<Label htmlFor="provider-id" className="text-[13px]">
								Provider ID
							</Label>
							<Input
								id="provider-id"
								ref={providerIdInput}
								aria-invalid={error?.field === "provider-id"}
								aria-describedby={error?.field === "provider-id" ? errorId : undefined}
								placeholder="my-provider"
								value={providerId}
								onChange={(e) => setProviderId(e.target.value)}
								disabled={isEdit || saving}
							/>
							<p className="text-[12px] text-muted-foreground">
								Lowercase letters, numbers, hyphens, or underscores. Avoid built-in ids unless you
								intend to overlay that template.
							</p>
						</div>

						{/* Display name */}
						<div className="flex flex-col gap-1.5">
							<Label htmlFor="display-name" className="text-[13px]">
								Display Name
							</Label>
							<Input
								id="display-name"
								placeholder={providerId || "My Provider"}
								value={displayName}
								onChange={(e) => setDisplayName(e.target.value)}
								disabled={saving}
							/>
						</div>

						{/* Base URL */}
						<div className="flex flex-col gap-1.5">
							<Label htmlFor="base-url" className="text-[13px]">
								Base URL
							</Label>
							<Input
								id="base-url"
								ref={baseUrlInput}
								aria-invalid={error?.field === "base-url"}
								aria-describedby={error?.field === "base-url" ? errorId : undefined}
								placeholder="https://api.example.com/v1"
								value={baseUrl}
								onChange={(e) => setBaseUrl(e.target.value)}
								disabled={saving}
							/>
						</div>

						{/* Wire API */}
						<div className="flex flex-col gap-1.5">
							<Label className="text-[13px]">Wire API</Label>
							<Select
								value={wireApi}
								items={Object.fromEntries(
									WIRE_API_OPTIONS.map((option) => [option.value, option.label]),
								)}
								onValueChange={(v) => setWireApi(v as CatalogWireApi)}
							>
								<SelectTrigger aria-label="Wire API">
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									<SelectGroup>
										{WIRE_API_OPTIONS.map((opt) => (
											<SelectItem key={opt.value} value={opt.value}>
												{opt.label}
											</SelectItem>
										))}
									</SelectGroup>
								</SelectContent>
							</Select>
						</div>

						{/* API key */}
						<div className="flex flex-col gap-1.5">
							<Label htmlFor="api-key" className="text-[13px]">
								API Key
							</Label>
							<Input
								id="api-key"
								type="password"
								placeholder={isEdit ? "(unchanged)" : "sk-…"}
								value={apiKey}
								onChange={(e) => setApiKey(e.target.value)}
								disabled={saving}
							/>
							<p className="text-[12px] text-muted-foreground">
								Optional for local servers that do not require authentication, or when using request
								headers.
							</p>
						</div>

						{/* Models */}
						<div className="flex flex-col gap-2">
							<Label className="text-[13px]">Models</Label>
							{isEdit && <p className="text-[12px] text-muted-foreground">Remove saved models from connection details.</p>}
							{models.map((m, i) => (
								<div key={i} className="flex items-center gap-2">
									<Input
										placeholder="model-id"
										value={m.id}
										onChange={(e) => updateModel(i, "id", e.target.value)}
										className="flex-1"
										disabled={saving || !!existing?.models?.[m.id]}
									/>
									<Input
										placeholder="Display name"
										value={m.name}
										onChange={(e) => updateModel(i, "name", e.target.value)}
										className="flex-1"
										disabled={saving}
									/>
									<Button
										variant="ghost"
										size="sm"
										onClick={() => removeModel(i)}
										disabled={saving || models.length <= 1 || !!existing?.models?.[m.id]}
									>
										<Trash2Icon className="size-3.5 stroke-[1.5]" />
									</Button>
								</div>
							))}
							<Button
								variant="ghost"
								size="sm"
								className="self-start text-[13px]"
								onClick={addModel}
								disabled={saving}
							>
								<PlusIcon className="size-3.5 stroke-[1.5]" />
								Add model
							</Button>
						</div>

						{/* Headers */}
						<div className="flex flex-col gap-2">
							<Label className="text-[13px]">Request Headers (optional)</Label>
							{headers.map((h, i) => (
								<div key={i} className="flex items-center gap-2">
									<Input
										placeholder="Header-Name"
										value={h.key}
										onChange={(e) => updateHeader(i, "key", e.target.value)}
										className="flex-1"
										disabled={saving}
									/>
									<Input
										placeholder="value"
										value={h.value}
										onChange={(e) => updateHeader(i, "value", e.target.value)}
										className="flex-1"
										disabled={saving}
									/>
									<Button
										variant="ghost"
										size="sm"
										onClick={() => removeHeader(i)}
										disabled={saving}
									>
										<Trash2Icon className="size-3.5 stroke-[1.5]" />
									</Button>
								</div>
							))}
							<Button
								variant="ghost"
								size="sm"
								className="self-start text-[13px]"
								onClick={addHeader}
								disabled={saving}
							>
								<PlusIcon className="size-3.5 stroke-[1.5]" />
								Add header
							</Button>
						</div>
					</div>
					{error && (
						<p
							id={errorId}
							role="alert"
							className="max-h-24 shrink-0 overflow-y-auto break-words text-[13px] text-destructive"
						>
							{error.message}
						</p>
					)}

					<DialogFooter className="shrink-0 border-t border-border/60 pt-3">
						<Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>
							Cancel
						</Button>
						<Button onClick={handleSave} disabled={saving}>
							{saving && <Spinner className="size-3.5" />}
							{saving ? "Saving…" : isEdit ? "Save" : "Add Provider"}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>

			<Dialog
				open={builtinConflictOpen}
				onOpenChange={(next) => {
					if (!saving) setBuiltinConflictOpen(next)
				}}
			>
				<DialogContent className="sm:max-w-lg gap-5 p-5">
					<DialogHeader className="gap-1.5">
						<DialogTitle>Built-in provider ID</DialogTitle>
						<DialogDescription>
							<code className="text-foreground">{providerId.trim()}</code> is a built-in provider.
							Overlaying merges with that template (same Connection id). Or pick another id such as{" "}
							<code className="text-foreground">{suggestedAlternateId}</code>.
						</DialogDescription>
					</DialogHeader>
					<DialogFooter className="flex-col gap-2 sm:flex-row sm:justify-end">
						<Button
							variant="outline"
							onClick={() => setBuiltinConflictOpen(false)}
							disabled={saving}
						>
							Cancel
						</Button>
						<Button variant="outline" onClick={handleUseSuggestedId} disabled={saving}>
							Use {suggestedAlternateId}
						</Button>
						<Button onClick={handleOverlayBuiltin} disabled={saving}>
							{saving && <Spinner className="size-3.5" />}
							Overlay built-in
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>
		</>
	)
}
