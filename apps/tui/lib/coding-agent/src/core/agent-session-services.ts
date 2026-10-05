export interface AgentSessionRuntimeDiagnostic { [key: string]: unknown }
export interface CreateAgentSessionServicesOptions { [key: string]: unknown }
export interface AgentSessionCreationOptions { [key: string]: unknown }
export interface CreateAgentSessionFromServicesOptions extends AgentSessionCreationOptions { [key: string]: unknown }
export interface AgentSessionServices {
	settingsManager: unknown;
	modelRegistry: unknown;
	resourceLoader: { getThemes(): { themes: unknown[] } };
	mcpManager: { refresh(): void };
}
export async function createAgentSessionServices(): Promise<AgentSessionServices> {
	return {
		settingsManager: undefined,
		modelRegistry: undefined,
		resourceLoader: { getThemes: () => ({ themes: [] }) },
		mcpManager: { refresh() {} },
	};
}
export async function createAgentSessionFromServices(): Promise<{ session: undefined }> {
	return { session: undefined };
}
