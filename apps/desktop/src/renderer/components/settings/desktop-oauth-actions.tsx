import { Button } from "@devo/ui/components/button"
import { useState } from "react"
import type { CatalogProvider } from "../../hooks/use-devo-data"
import { ConnectProviderDialog } from "./connect-provider-dialog"
import { desktopOAuthActions } from "./desktop-oauth-providers"

/** Account login adapters stay available independently of directory availability. */
export function DesktopOAuthActions({ onConnected }: { onConnected: () => void }) {
	const [provider, setProvider] = useState<CatalogProvider | null>(null)
	return (
		<section aria-label="Account sign-in" className="space-y-3 rounded-lg border border-border/60 p-4">
			<div>
				<h3 className="text-sm font-medium">Account sign-in</h3>
				<p className="mt-1 text-xs text-muted-foreground">
					Connect an existing subscription. Available models come from the live directory or your configuration.
				</p>
			</div>
			<div className="flex flex-wrap gap-2">
				{desktopOAuthActions.map(({ providerId, label }) => (
					<Button key={providerId} variant="outline" size="sm" onClick={() => {
						// Dialog-only identity. Never merge adapter metadata into the catalog.
						setProvider({
							id: providerId,
							name: label.replace(/^Sign in with /, ""),
							env: [],
							models: {},
							wireApis: [],
							enabled: true,
						})
					}}>{label}</Button>
				))}
			</div>
			{provider && <ConnectProviderDialog provider={provider}
				onClose={() => setProvider(null)}
				onConnected={() => { setProvider(null); onConnected() }} />}
		</section>
	)
}
