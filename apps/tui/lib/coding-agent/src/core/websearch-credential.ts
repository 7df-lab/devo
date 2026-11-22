export const BUILTIN_WEB_CREDENTIAL_IDS = ["tavily", "deepseek", "serper", "exa"] as const;
export type BuiltinWebCredentialId = (typeof BUILTIN_WEB_CREDENTIAL_IDS)[number];

// Existing auth.json IDs: keys saved for legacy integrations remain usable.
export const WEB_SERVICE_CREDENTIALS = new Map<string, string>([
	["serper", "Serper (web search and fetch)"],
	["exa", "Exa (web search and fetch)"],
	["tavily", "Tavily (web search and fetch)"],
]);
