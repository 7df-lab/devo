import { Text, TruncatedText, type Component } from "@earendil-works/pi-tui";
import { AGENT_MESSAGE_RECEIVED_PREVIEW_LABEL } from "../../core/agent-messages.js";
import { GOAL_CONTEXT_PREVIEW_LABEL } from "../../core/goals.js";
import {
	ASYNC_BASH_COMPLETION_PREVIEW_LABEL,
	SCHEDULED_PROMPT_PREVIEW_LABEL,
} from "../../core/messages.js";
import type { OrderedQueuedMessage } from "../../core/session-action-store.js";
import { isLeadingSlashCommand, styleSlashCommandText } from "./components/slash-command-message.js";
import { styleArgumentTokens } from "./components/prompt-highlight.js";
import { theme } from "./theme/theme.js";

function isLabeledQueuedPreview(message: string): boolean {
	return (
		message.startsWith(`${SCHEDULED_PROMPT_PREVIEW_LABEL}: `) ||
		message.startsWith(`${GOAL_CONTEXT_PREVIEW_LABEL}: `) ||
		message.startsWith(`${AGENT_MESSAGE_RECEIVED_PREVIEW_LABEL}: `) ||
		message.startsWith(`${ASYNC_BASH_COMPLETION_PREVIEW_LABEL}: `)
	);
}

export function formatQueuedMessagePreview(message: string, label: string): string {
	return isLabeledQueuedPreview(message) ? message : `${label}: ${message}`;
}

export function styleQueuedMessagePreview(
	message: string,
	label: string,
	isRecognizedSlashCommand: (name: string) => boolean,
): string {
	const preview = formatQueuedMessagePreview(message, label);
	const styleDim = (segment: string) => theme.fg("dim", segment);
	if (!isLeadingSlashCommand(message, isRecognizedSlashCommand)) return styleArgumentTokens(preview, styleDim);
	const prefix = preview.slice(0, preview.length - message.length);
	return `${theme.fg("dim", prefix)}${styleSlashCommandText(message, (rest, includeBareSeparator) =>
		styleArgumentTokens(rest, styleDim, includeBareSeparator),
	)}`;
}

/** Bounds the dock on each render; browsing still uses the complete queue. */
export class QueuedMessagesPreview implements Component {
	constructor(
		private readonly entries: readonly OrderedQueuedMessage[],
		private readonly options: {
			numbered: boolean;
			browseKey: string;
			getRows: () => number;
			isRecognizedSlashCommand: (name: string) => boolean;
		},
	) {}

	invalidate(): void {}

	render(width: number): string[] {
		if (!this.entries.length) return [];
		const rows = this.options.getRows();
		const summaryOnly = rows < 12;
		const previewLimit = summaryOnly ? 0 : rows < 20 ? 2 : 4;
		const visible = this.entries.slice(0, previewLimit);
		const hidden = this.entries.length - visible.length;
		const held = this.entries.filter((entry) => entry.recoveryRequired);
		const oneLine = (text: string) => new TruncatedText(text, 1, 0).render(width);
		const lines = rows >= 20 ? [""] : [];
		for (const [index, entry] of visible.entries()) {
			const mode = entry.recoveryRequired ? "Held · review required" : entry.delivery === "steer" ? "Steering" : "Follow-up";
			const label = this.options.numbered ? `${index + 1}. ${mode}${entry.blocked ? " · waiting behind earlier input" : ""}` : mode;
			lines.push(...oneLine(styleQueuedMessagePreview(entry.preview ?? entry.text, label, this.options.isRecognizedSlashCommand)));
		}
		if (held.length && !summaryOnly) {
			const subject = held.length === 1 ? "Held input" : `${held.length} held inputs`;
			if (rows >= 20 && width >= 72) {
				const label = held.length === 1 ? `${subject} ${held[0].id}` : subject;
				const warning = `${label} may already have partially executed after interruption, recovery, or a turn limit. Browse and remove to discard. Explicit retry through Native session/queue/retry requires reviewing the prior turn and may be rejected while the turn limit applies; there is no automatic retry.`;
				lines.push(...new Text(theme.fg("warning", warning), 1, 0).render(width));
			} else {
				lines.push(...oneLine(theme.fg("warning", `${subject} may have partially run · no auto retry`)));
			}
		}
		const count = summaryOnly ? `${hidden} queued${held.length ? ` (${held.length} held)` : ""} · ` : hidden ? `+${hidden} more · ` : "";
		const editHint = this.entries.some((entry) => entry.editable !== false) ? " and edit" : "";
		lines.push(...oneLine(theme.fg("dim", `╰─ ${count}${this.options.browseKey} to browse${editHint}${count ? "" : " queued messages"}`)));
		return lines;
	}
}
