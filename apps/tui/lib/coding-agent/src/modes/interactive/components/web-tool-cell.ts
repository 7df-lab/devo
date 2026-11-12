import { type Component, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { theme } from "../theme/theme.js";
import { getWorkingPulseFrame, workingIconFrame } from "../theme/working-icon.js";
import { expandedToolOutput, formatCollapsedToolPreview, summarizeToolArgs, truncateExpandedToolOutput } from "./tool-execution-preview.js";

export function isWebTool(name: string): boolean {
	return /^(web[_-]?search|web[_-]?fetch|fetch_url)$/i.test(name);
}

interface WebToolCellState {
	name: string;
	args: unknown;
	output: string;
	details: unknown;
	expanded: boolean;
	status: "queued" | "running" | "done" | "error";
}

/** Web calls use the same quiet status row and indented detail as Python cells. */
export class WebToolCellComponent implements Component {
	constructor(private readonly state: WebToolCellState) {}
	invalidate(): void {}

	render(width: number): string[] {
		const safeWidth = Math.max(1, width);
		const { name, args, output, details, expanded, status } = this.state;
		const marker = status === "running"
			? theme.fg("bashMode", workingIconFrame(getWorkingPulseFrame()))
			: status === "error" ? theme.fg("error", "✗")
			: status === "done" ? theme.fg("success", "✓") : theme.fg("muted", "◇");
		const summary = summarizeToolArgs(name, args);
		const parts = [`${marker} ${theme.fg("muted", name)}`];
		if (summary) parts.push(theme.fg("dim", summary));
		else if (status === "queued" || status === "running") parts.push(theme.fg("dim", "waiting for arguments"));
		const header = ` ${parts.join(theme.fg("dim", " · "))}`;
		const preview = output.trim()
			? theme.fg(status === "error" ? "error" : "dim", formatCollapsedToolPreview(output)) : "";
		const combined = preview ? `${header}${theme.fg("dim", " · ")}${preview}` : header;
		// A long query/URL must never consume the result or failure preview. Keep
		// short calls on one row, and give overflowing output one bounded row of its own.
		const lines = preview && visibleWidth(combined) > safeWidth
			? [truncateToWidth(header, safeWidth, "…"), truncateToWidth(` ${theme.fg("dim", " › ")}${preview}`, safeWidth, "…")]
			: [truncateToWidth(combined, safeWidth, "…")];
		if (!expanded) return lines;

		// Use full arguments on expansion; the header's bounded preview is not a substitute.
		const input = typeof args === "string" ? args : args && typeof args === "object"
			? Object.entries(args).map(([key, value]) => `${key}: ${typeof value === "string" ? value : JSON.stringify(value)}`).join("\n") : "";
		const result = truncateExpandedToolOutput(expandedToolOutput(output, details));
		for (const [text, gutter] of [[input, "╰─ "], [result, " › "]] as const) {
			if (!text) continue;
			if (gutter === " › " && input) lines.push("");
			let first = true;
			for (const line of text.split("\n")) {
				const inset = " ".slice(0, Math.max(0, safeWidth - 1));
				const prefix = (first ? gutter : "   ").slice(0, Math.max(0, safeWidth - inset.length - 1));
				for (const [index, wrapped] of wrapTextWithAnsi(theme.fg(status === "error" ? "error" : "toolOutput", line), Math.max(1, safeWidth - inset.length - prefix.length)).entries()) {
					lines.push(truncateToWidth(`${inset}${theme.fg("dim", index === 0 ? prefix : " ".repeat(prefix.length))}${wrapped}`, safeWidth, ""));
				}
				first = false;
			}
		}
		return lines;
	}
}
