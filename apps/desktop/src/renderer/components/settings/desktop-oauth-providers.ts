export interface ProviderAuthMethod {
	type: "api" | "oauth"
	label: string
}

const DESKTOP_OAUTH_METHODS: Record<string, ProviderAuthMethod[]> = {
	"openai-codex": [{ type: "oauth", label: "Sign in with ChatGPT" }],
	anthropic: [
		{ type: "oauth", label: "Sign in with Claude Pro/Max" },
		{ type: "api", label: "API Key" },
	],
	"github-copilot": [{ type: "oauth", label: "Sign in with GitHub Copilot" }],
	xai: [
		{ type: "oauth", label: "Sign in with xAI" },
		{ type: "api", label: "API Key" },
	],
}

export function isDesktopOAuthProvider(providerId: string): boolean {
	return providerId in DESKTOP_OAUTH_METHODS
}

/** Settings Connect: OAuth builtins use ConnectProviderDialog; others use API-key template. */
export function settingsConnectDialogKind(
	providerId: string,
): "oauth" | "api-key-template" {
	return isDesktopOAuthProvider(providerId) ? "oauth" : "api-key-template"
}

export function resolveProviderAuthMethods(
	providerId: string,
	pluginMethods?: ProviderAuthMethod[],
): ProviderAuthMethod[] {
	return DESKTOP_OAUTH_METHODS[providerId] ?? pluginMethods ?? [{ type: "api", label: "API Key" }]
}
