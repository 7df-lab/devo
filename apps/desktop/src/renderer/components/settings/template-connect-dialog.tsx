/**
 * Dialog for connecting a built-in provider template.
 *
 * The template supplies defaults for endpoint and protocol. The user can
 * override the Base URL and supply an API key before confirming.
 */

import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Label } from "@devo/ui/components/label"
import { Spinner } from "@devo/ui/components/spinner"
import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
import { CheckCircle2Icon } from "lucide-react"
import { useCallback, useId, useLayoutEffect, useRef, useState } from "react"
import { PROVIDER_KEY_URLS, PROVIDERS_OPTIONAL_API_KEY } from "../../lib/providers"
import { getBaseClient } from "../../services/connection-manager"
import { ProviderIcon } from "./provider-icon"

interface TemplateConnectDialogProps {
	provider: CatalogProviderInfo
	open: boolean
	onOpenChange: (open: boolean) => void
	onConnected: () => void
}

type ConnectState =
	| { status: "idle" }
	| { status: "saving" }
	| { status: "success"; discoveryFailed: boolean }
	| { status: "error"; message: string; field?: "base-url" | "api-key" }

export function TemplateConnectDialog({
	provider,
	open,
	onOpenChange,
	onConnected,
}: TemplateConnectDialogProps) {
	const [apiKey, setApiKey] = useState("")
	const [baseUrl, setBaseUrl] = useState(provider.baseUrl ?? "")
	const [state, setState] = useState<ConnectState>({ status: "idle" })

	const errorId = useId()
	const apiKeyInput = useRef<HTMLInputElement>(null)
	const baseUrlInput = useRef<HTMLInputElement>(null)
	const generation = useRef(0)
	const saving = useRef(false)

	useLayoutEffect(() => {
		generation.current += 1
		saving.current = false
		setApiKey("")
		setBaseUrl(provider.baseUrl ?? "")
		setState({ status: "idle" })
		return () => {
			generation.current += 1
		}
	}, [open, provider.baseUrl, provider.id])

	const keyUrl = PROVIDER_KEY_URLS[provider.id]
	const apiKeyOptional = PROVIDERS_OPTIONAL_API_KEY.has(provider.id)
	const busy = state.status === "saving"
	const saved = state.status === "success"
	const trimmedBaseUrl = baseUrl.trim()

	const handleConnect = useCallback(async () => {
		if (!open || saving.current || saved) return
		if (!trimmedBaseUrl) {
			setState({ status: "error", message: "Base URL is required", field: "base-url" })
			baseUrlInput.current?.focus()
			return
		}
		if (!apiKeyOptional && !apiKey.trim()) {
			setState({ status: "error", message: "API key is required", field: "api-key" })
			apiKeyInput.current?.focus()
			return
		}
		const requestGeneration = generation.current
		saving.current = true
		setState({ status: "saving" })
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			// Local runtimes like Ollama must not seed the Connection with
			// bundled placeholder model ids — those 404 on chat. Connect empty
			// and refresh from the live directory instead.
			const seedLiveDirectory = apiKeyOptional
			const connectionProvider: CatalogProviderInfo = {
				...provider,
				baseUrl: trimmedBaseUrl,
				...(seedLiveDirectory ? { models: {} } : {}),
			}
			await client.provider.upsert({
				provider: connectionProvider,
				...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
			})
			if (generation.current !== requestGeneration) return
			let discoveryFailed = false
			if (seedLiveDirectory) {
				try {
					await client.provider.discover({
						providerId: provider.id,
						forceRefresh: true,
					})
				} catch {
					// The configuration was saved, but directory access was not verified.
					discoveryFailed = true
				}
			}
			if (generation.current !== requestGeneration) return
			setState({ status: "success", discoveryFailed })
		} catch (err) {
			if (generation.current !== requestGeneration) return
			setState({
				status: "error",
				message: err instanceof Error ? err.message : "Failed to save configuration",
			})
		} finally {
			if (generation.current === requestGeneration) saving.current = false
		}
	}, [provider, apiKey, apiKeyOptional, trimmedBaseUrl, open, saved])

	const handleOpenChange = (next: boolean) => {
		if (saving.current) return
		if (!next && saved) onConnected()
		onOpenChange(next)
	}

	return (
		<Dialog open={open} onOpenChange={handleOpenChange}>
			<DialogContent className="sm:max-w-lg gap-0 p-0 overflow-hidden" showCloseButton={!busy}>
				<div className="border-b border-border/50 px-5 py-4">
					<DialogHeader className="gap-2">
						<div className="flex items-center gap-2.5">
							<ProviderIcon id={provider.id} name={provider.name} size="sm" />
							<DialogTitle className="text-base font-medium tracking-tight">
								Connect {provider.name}
							</DialogTitle>
						</div>
						<DialogDescription className="text-sm leading-5">
							Creates a Connection from this template. You can override the Base URL; model settings
							are configured under Models. Saving does not test model access.
							{PROVIDERS_OPTIONAL_API_KEY.has(provider.id)
								? " Local providers refresh the model list from the live directory on connect."
								: null}
						</DialogDescription>
					</DialogHeader>
				</div>

				<div className="flex flex-col gap-4 px-5 py-4">
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="connect-base-url" className="text-xs text-muted-foreground">
							Base URL
						</Label>
						<Input
							id="connect-base-url"
							ref={baseUrlInput}
							aria-invalid={state.status === "error" && state.field === "base-url"}
							aria-describedby={
								state.status === "error" && state.field === "base-url" ? errorId : undefined
							}
							value={baseUrl}
							onChange={(e) => setBaseUrl(e.target.value)}
							placeholder={provider.baseUrl ?? "https://…"}
							disabled={busy || saved}
							className="h-9"
						/>
					</div>

					<div className="flex flex-col gap-1.5">
						<Label htmlFor="api-key" className="text-xs text-muted-foreground">
							API key
						</Label>
						<Input
							id="api-key"
							ref={apiKeyInput}
							required={!apiKeyOptional}
							aria-invalid={state.status === "error" && state.field === "api-key"}
							aria-describedby={
								state.status === "error" && state.field === "api-key" ? errorId : undefined
							}
							type="password"
							placeholder={apiKeyOptional ? "optional" : "sk-…"}
							value={apiKey}
							onChange={(e) => setApiKey(e.target.value)}
							disabled={busy || saved}
							className="h-9"
						/>
						{keyUrl && (
							<a
								href={keyUrl.url}
								target="_blank"
								rel="noopener noreferrer"
								className="text-xs text-muted-foreground underline-offset-4 hover:underline"
							>
								{keyUrl.label} →
							</a>
						)}
					</div>

					{state.status === "error" && (
						<p id={errorId} role="alert" className="text-sm text-destructive">
							{state.message}
						</p>
					)}
					{state.status === "success" && (
						<output className="flex items-start gap-2 text-sm text-muted-foreground">
							<CheckCircle2Icon className="mt-0.5 size-3.5 shrink-0 stroke-[1.5]" />
							<span>
								Configuration saved. Model access has not been tested.
								{state.discoveryFailed
									? " Could not refresh models. Retry Discover in the connection details."
									: null}
							</span>
						</output>
					)}
				</div>

				<div className="flex items-center justify-end gap-2 border-t border-border/50 px-5 py-3">
					{saved ? (
						<Button size="sm" onClick={() => handleOpenChange(false)}>
							Done
						</Button>
					) : (
						<>
							<Button
								variant="outline"
								size="sm"
								onClick={() => handleOpenChange(false)}
								disabled={busy}
							>
								Cancel
							</Button>
							<Button size="sm" onClick={handleConnect} disabled={busy}>
								{busy && <Spinner className="size-3.5" />}
								{busy ? "Saving…" : "Connect"}
							</Button>
						</>
					)}
				</div>
			</DialogContent>
		</Dialog>
	)
}
