// ============================================================
// Tool categories — used by grouping and metrics.
// ============================================================

export type ToolCategory =
	| "explore"
	| "edit"
	| "run"
	| "delegate"
	| "plan"
	| "ask"
	| "fetch"
	| "other"

export function getToolCategory(tool: string): ToolCategory {
	switch (tool) {
		case "read":
		case "glob":
		case "grep":
		case "list":
			return "explore"
		case "edit":
		case "write":
		case "apply_patch":
			return "edit"
		case "bash":
		case "shell_command":
		case "exec_command":
			return "run"
		case "task":
			return "delegate"
		case "todowrite":
		case "todoread":
			return "plan"
		case "question":
		case "request_user_input":
			return "ask"
		case "webfetch":
			return "fetch"
		default:
			return "other"
	}
}
