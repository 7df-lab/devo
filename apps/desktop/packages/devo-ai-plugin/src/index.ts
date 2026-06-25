export interface PluginInput {
	name?: string
	description?: string
	command?: string
	args?: string[]
	env?: Record<string, string>
	[key: string]: unknown
}
