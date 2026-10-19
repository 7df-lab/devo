import { AGENT_MESSAGE_RECEIVED_PREVIEW_LABEL } from "../../core/agent-messages.js";
import { GOAL_CONTEXT_PREVIEW_LABEL } from "../../core/goals.js";
import {
	ASYNC_BASH_COMPLETION_PREVIEW_LABEL,
	HEARTBEAT_PROMPT_PREVIEW_LABEL,
} from "../../core/messages.js";
import { isLeadingSlashCommand, styleSlashCommandText } from "./components/slash-command-message.js";
import { styleArgumentTokens } from "./components/prompt-highlight.js";
import { theme } from "./theme/theme.js";

function isLabeledQueuedPreview(message: string): boolean {
	return (
		message.startsWith(`${HEARTBEAT_PROMPT_PREVIEW_LABEL}: `) ||
		message.startsWith(`${GOAL_CONTEXT_PREVIEW_LABEL}: `) ||
		message.startsWith(`${AGENT_MESSAGE_RECEIVED_PREVIEW_LABEL}: `) ||
		message.startsWith(`${ASYNC_BASH_COMPLETION_PREVIEW_LABEL}: `)
	);
}

export function formatQueuedMessagePreview(message: string, label: "Steering" | "Follow-up"): string {
	return isLabeledQueuedPreview(message) ? message : `${label}: ${message}`;
}

export function styleQueuedMessagePreview(
	message: string,
	label: "Steering" | "Follow-up",
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
