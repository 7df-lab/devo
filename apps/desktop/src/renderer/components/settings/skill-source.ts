const SOURCE_LABELS: Record<string, string> = {
	User: "user",
	Workspace: "workspace",
	Plugin: "plugin",
	System: "system",
	Admin: "admin",
}

export function skillSourceLabel(source: unknown): string {
	if (typeof source === "string") return SOURCE_LABELS[source] ?? source
	if (source && typeof source === "object") {
		const value = source as Record<string, unknown>
		if ("User" in value) return "user"
		if (typeof value.Workspace === "object" || "cwd" in value) return "workspace"
		if (typeof value.Plugin === "object" || "plugin_id" in value || "pluginId" in value) {
			return "plugin"
		}
		if ("System" in value) return "system"
		if ("Admin" in value) return "admin"
	}
	return "unknown"
}
