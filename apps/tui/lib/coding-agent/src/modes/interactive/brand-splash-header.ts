import * as os from "node:os";
import type { Component } from "@earendil-works/pi-tui";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
const DEFAULT_BRAND_LOGO = `██████╗  ███████╗██╗   ██╗ ██████╗
██╔══██╗ ██╔════╝██║   ██║██╔═══██╗
██║  ██║ █████╗  ██║   ██║██║   ██║
██║  ██║ ██╔══╝  ╚██╗ ██╔╝██║   ██║
██████╔╝ ███████╗ ╚████╔╝ ╚██████╔╝
╚═════╝  ╚══════╝  ╚═══╝   ╚═════╝`;

import type { AgentConnectionRlmChildAgentSnapshot } from "../agent-connection/types.js";
import { theme } from "./theme/theme.js";

export function formatSplashCwd(cwd: string): string {
	const normalized = cwd.replace(/\\/g, "/");
	const home = os.homedir().replace(/\\/g, "/");
	if (home && normalized === home) {
		return "~";
	}
	if (home && normalized.startsWith(`${home}/`)) {
		return `~${normalized.slice(home.length)}`;
	}

	return normalized;
}

export function mergeSubagentSnapshot(
	previous: AgentConnectionRlmChildAgentSnapshot,
	incoming: AgentConnectionRlmChildAgentSnapshot,
): AgentConnectionRlmChildAgentSnapshot {
	const active = incoming.status === "running" || incoming.status === "queued";
	return {
		...previous,
		...incoming,
		parentId: incoming.parentId ?? previous.parentId,
		// Active updates may omit a previously known daemon session id, but a
		// terminal update without one means the child is no longer resident.
		activeSessionId: active ? (incoming.activeSessionId ?? previous.activeSessionId) : incoming.activeSessionId,
		// A completed retained child can become active again when it receives a
		// follow-up. Its RLM run status stays terminal, so activity must remain an
		// independent projection of the live session state.
		activity: active ? (incoming.activity ?? previous.activity) : incoming.activity,
	};
}

export function truncatePathMiddle(value: string, width: number): string {
	if (visibleWidth(value) <= width) {
		return value;
	}
	if (width <= 1) {
		return truncateToWidth(value, width, "");
	}

	const ellipsis = "…";
	const normalized = value.replace(/\\/g, "/");
	const prefix = normalized.startsWith("~/") ? "~/" : normalized.startsWith("/") ? "/" : "";
	const body = prefix ? normalized.slice(prefix.length) : normalized;
	const parts = body.split("/").filter((part) => part.length > 0);
	const last = parts.pop() ?? "";
	const previous = parts.pop();
	const suffix = previous ? `${previous}/${last}` : last;
	const candidate = `${prefix}${ellipsis}/${suffix}`;
	if (visibleWidth(candidate) <= width) {
		return candidate;
	}

	return truncateToWidth(candidate, width);
}

export interface BrandSplashMetadataLine {
	label: string;
	value: string;
}

export interface BrandSplashHeaderOptions {
	logo?: string;
	/** Product name shown beside the logo. Defaults to "devo". */
	title?: string;
	topPadding?: boolean;
	getModelId?: () => string | undefined;
	getExtraMetadata?: () => readonly BrandSplashMetadataLine[];
}

export class BrandSplashHeader implements Component {
	private readonly logoRaw: string[];
	private readonly logoCanvasWidth: number;
	private readonly gutter = 3;

	constructor(
		private readonly version: string,
		private readonly getCwd: () => string | undefined,
		private readonly verboseInstructions?: string,
		private readonly options: BrandSplashHeaderOptions = {},
	) {
		this.logoRaw = (options.logo ?? DEFAULT_BRAND_LOGO).split("\n");
		this.logoCanvasWidth = this.logoRaw.reduce((max, line) => Math.max(max, visibleWidth(line)), 0);
	}

	invalidate(): void {
		// Render output is derived from current theme/session state.
	}

	render(width: number): string[] {
		const safeWidth = Math.max(1, width);
		const paddingX = safeWidth > 1 ? 1 : 0;
		const contentWidth = Math.max(1, safeWidth - paddingX * 2);
		const showLogo = this.logoCanvasWidth > 0 && contentWidth - this.logoCanvasWidth - this.gutter >= 24;
		const metaWidth = showLogo ? contentWidth - this.logoCanvasWidth - this.gutter : contentWidth;
		const extraMetadata = this.options.getExtraMetadata?.() ?? [];
		const version = theme.fg("muted", `v${this.version}`);
		const titleText = this.options.title ?? "devo";
		const title = theme.fg("text", titleText);
		const modelLabel = "model ";
		const cwdLabel = "cwd ";
		const cwd = this.getCwd();
		const metaLines = [
			...(visibleWidth(`${titleText} v${this.version}`) <= metaWidth ? [`${title} ${version}`] : [title, version]),
			...(this.options.getModelId
				? [
						`${theme.fg("dim", modelLabel)}${theme.fg(
							"muted",
							truncateToWidth(
								this.options.getModelId() ?? "—",
								Math.max(1, metaWidth - visibleWidth(modelLabel)),
							),
						)}`,
					]
				: []),
			...extraMetadata.map(({ label, value }) => `${theme.fg("dim", `${label} `)}${theme.fg("muted", value)}`),
			...(cwd === undefined
				? []
				: [
						`${theme.fg("dim", cwdLabel)}${theme.fg("muted", truncatePathMiddle(formatSplashCwd(cwd), Math.max(1, metaWidth - visibleWidth(cwdLabel))))}`,
					]),
		];
		const lines = this.options.topPadding ? [""] : [];
		const rowCount = Math.max(showLogo ? this.logoRaw.length : 0, metaLines.length);
		const metaStartIndex = showLogo ? Math.floor((rowCount - metaLines.length) / 2) : 0;
		for (let index = 0; index < rowCount; index++) {
			const logoLine = showLogo ? (this.logoRaw[index] ?? "") : "";
			const logo = showLogo
				? theme.fg("text", logoLine) + " ".repeat(this.logoCanvasWidth - visibleWidth(logoLine) + this.gutter)
				: "";
			const metaIndex = index - metaStartIndex;
			const metaLine = metaIndex >= 0 && metaIndex < metaLines.length ? metaLines[metaIndex] : "";
			const content = truncateToWidth(logo + metaLine, contentWidth);
			lines.push(
				" ".repeat(paddingX) + content + " ".repeat(Math.max(0, safeWidth - paddingX - visibleWidth(content))),
			);
		}

		if (this.verboseInstructions) {
			lines.push(" ".repeat(safeWidth));
			for (const instruction of this.verboseInstructions.split("\n")) {
				const content = truncateToWidth(instruction, contentWidth);
				lines.push(
					" ".repeat(paddingX) + content + " ".repeat(Math.max(0, safeWidth - paddingX - visibleWidth(content))),
				);
			}
		}

		return lines;
	}
}
