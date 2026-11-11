const COLLAPSED_TOOL_PREVIEW_CHARS = 240;
const COLLAPSED_TOOL_PREVIEW_LINES = 2;
const EXPANDED_TOOL_PREVIEW_CHARS = 12_000;
const EXPANDED_TOOL_PREVIEW_LINES = 120;

/** One-line arg preview for common tools (especially web_search query). */
export function summarizeToolArgs(toolName: string, args: unknown): string {
	if (!args || typeof args !== "object") return "";
	const record = args as Record<string, unknown>;
	const name = toolName.toLowerCase();
	const pick = (...keys: string[]): string => {
		for (const key of keys) {
			const value = record[key];
			if (typeof value === "string" && value.trim()) {
				return value.trim().replace(/\s+/g, " ");
			}
		}
		return "";
	};

	if (name === "web_search" || name === "websearch" || name === "web-search") {
		const action = record.action && typeof record.action === "object" ? record.action : record;
		const source = action as Record<string, unknown>;
		const queries = Array.isArray(source.queries)
			? source.queries.filter((query): query is string => typeof query === "string").join("; ")
			: "";
		const query = pick("query", "q", "search", "text", "url") ||
			(typeof source.query === "string" ? source.query : queries) ||
			(typeof source.url === "string" ? source.url : "");
		return query ? truncatePlain(query.trim().replace(/\s+/g, " "), 80) : "";
	}
	if (name === "web_fetch" || name === "webfetch" || name === "web-fetch" || name === "fetch_url") {
		const action = record.action && typeof record.action === "object" ? record.action as Record<string, unknown> : {};
		const url = pick("url", "uri", "href") || (typeof action.url === "string" ? action.url : "");
		return url ? truncatePlain(url.trim().replace(/\s+/g, " "), 80) : "";
	}
	if (name === "bash" || name === "shell_command" || name === "exec_command") {
		const command = pick("command", "cmd");
		return command ? truncatePlain(command, 80) : "";
	}
	if (name === "ipython") {
		const code = pick("code");
		return code ? truncatePlain(code.split(/\r?\n/).find((line) => line.trim()) ?? code, 80) : "";
	}
	const generic = pick("query", "url", "path", "command", "message", "prompt");
	return generic ? truncatePlain(generic, 80) : "";
}

export function formatCollapsedToolPreview(text: string): string {
	const lines = text.replace(/\r\n/g, "\n").split("\n").filter((line) => line.trim().length > 0);
	if (lines.length === 0) return "";
	const head = lines.slice(0, COLLAPSED_TOOL_PREVIEW_LINES).join("\n");
	const truncated = truncatePlain(head.replace(/\s+/g, " ").trim(), COLLAPSED_TOOL_PREVIEW_CHARS);
	const hidden = Math.max(0, lines.length - COLLAPSED_TOOL_PREVIEW_LINES);
	if (hidden > 0 || text.length > COLLAPSED_TOOL_PREVIEW_CHARS) {
		const more =
			hidden > 0
				? `… ${hidden} more lines`
				: `… ${Math.max(1, text.length - COLLAPSED_TOOL_PREVIEW_CHARS)} more chars`;
		return `${truncated} ${more}`;
	}
	return truncated;
}

export function truncateExpandedToolOutput(text: string): string {
	const lines = text.replace(/\r\n/g, "\n").split("\n");
	if (lines.length <= EXPANDED_TOOL_PREVIEW_LINES && text.length <= EXPANDED_TOOL_PREVIEW_CHARS) {
		return text;
	}
	const head = lines.slice(0, EXPANDED_TOOL_PREVIEW_LINES).join("\n");
	const clipped =
		head.length > EXPANDED_TOOL_PREVIEW_CHARS
			? `${head.slice(0, EXPANDED_TOOL_PREVIEW_CHARS).trimEnd()}…`
			: head;
	const hiddenLines = Math.max(0, lines.length - EXPANDED_TOOL_PREVIEW_LINES);
	return `${clipped}\n… truncated (${hiddenLines > 0 ? `${hiddenLines} more lines` : "output capped"})`;
}

function truncatePlain(text: string, maxChars: number): string {
	if (text.length <= maxChars) return text;
	return `${text.slice(0, Math.max(0, maxChars - 1)).trimEnd()}…`;
}

/** Restore readable details when a result was shortened for the conversation preview. */
export function expandedToolOutput(text: string, details: unknown): string {
	if (!details || typeof details !== "object") return text;
	const record = details as Record<string, unknown>;
	if (typeof record.fullText === "string") return record.fullText;
	if (record.resultKind !== "web_search" || !Array.isArray(record.hits)) return text;
	const hits = record.hits.slice(0, EXPANDED_TOOL_PREVIEW_LINES).flatMap((hit, index) => {
		if (!hit || typeof hit !== "object") return [];
		const result = hit as Record<string, unknown>;
		const title = String(result.title ?? result.name ?? result.url ?? `Result ${index + 1}`);
		const url = typeof result.url === "string" && result.url !== title ? result.url : "";
		const snippet = [result.snippet, result.description, result.text].find((value) => typeof value === "string") ?? "";
		return [[`${index + 1}. ${title}`, url, snippet].filter(Boolean).join("\n")];
	});
	const remaining = record.hits.length - hits.length;
	return [text, ...hits, remaining > 0 ? `… ${remaining} more results` : ""].filter(Boolean).join("\n\n");
}

