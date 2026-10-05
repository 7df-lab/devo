import type { AgentToolResult } from "@earendil-works/pi-agent-core";
import {
	type Component,
	Container,
	Image,
	Text,
	truncateToWidth,
	type TUI,
	visibleWidth,
	wrapTextWithAnsi,
} from "@earendil-works/pi-tui";
import type { ToolDefinition, ToolRenderContext, ToolRenderResultOptions } from "../../../core/extensions/types.js";
import type { KernelSentAgentMessage } from "../../../core/kernel/index.js";
import { createBashToolDefinition } from "../../../core/tools/bash.js";
import { createEditToolDefinition } from "../../../core/tools/edit.js";
import { createAllToolDefinitions } from "../../../core/tools/index.js";
import { getTextOutput as getRenderedTextOutput } from "../../../core/tools/render-utils.js";
import type { AgentConnectionToolDefinition } from "../../agent-connection/index.js";
import { type Theme, theme } from "../theme/theme.js";
import { getWorkingPulseFrame, workingIconFrame } from "../theme/working-icon.js";
import { getIpythonCodeFromArgs, IPythonCellComponent } from "./ipython-cell.js";
import {
	type BackgroundShellHandle,
	readAssignedShellCommand,
	readBackgroundShellHandle,
	type ShellCompletion,
} from "./shell-completion.js";
import { ToolPanel } from "./tool-panel.js";

export interface ToolExecutionOptions {
	showImages?: boolean;
	/** Whether image metadata may parse dimensions from base64 data. */
	includeImageDimensions?: boolean;
}

export interface ToolExecutionRendererDefinition {
	renderShell?: "default" | "self";
	renderCall?: (args: any, theme: Theme, context: ToolRenderContext<any, any>) => Component;
	renderResult?: (
		result: AgentToolResult<any>,
		options: ToolRenderResultOptions,
		theme: Theme,
		context: ToolRenderContext<any, any>,
	) => Component;
}
export type ToolExecutionDefinition = AgentConnectionToolDefinition & Partial<ToolExecutionRendererDefinition>;

function hasToolRenderer(toolDefinition: ToolExecutionDefinition | undefined): boolean {
	return toolDefinition?.renderCall !== undefined || toolDefinition?.renderResult !== undefined;
}

function matchesBuiltInReplayMetadata(toolName: string, toolDefinition: ToolExecutionDefinition | undefined): boolean {
	if (!toolDefinition) {
		return true;
	}
	if (hasToolRenderer(toolDefinition)) {
		return false;
	}
	return toolDefinition.replayBuiltInToolName === toolName;
}

function createReplayBuiltInToolDefinition(
	toolName: string,
	cwd: string,
	toolDefinition: ToolExecutionDefinition | undefined,
): ToolDefinition<any, any> | undefined {
	if (toolName === "ipython") {
		return createAllToolDefinitions(cwd).ipython;
	}
	switch (toolName) {
		case "bash": {
			const builtInDefinition = createBashToolDefinition(cwd);
			return matchesBuiltInReplayMetadata(toolName, toolDefinition) ? builtInDefinition : undefined;
		}
		case "edit": {
			const builtInDefinition = createEditToolDefinition(cwd);
			return matchesBuiltInReplayMetadata(toolName, toolDefinition) ? builtInDefinition : undefined;
		}
		default:
			return undefined;
	}
}

type PlanCellStatus = "completed" | "inProgress" | "pending";

interface PlanCellEntry {
	step: string;
	status: PlanCellStatus;
}

function readPlanCellEntries(value: unknown): PlanCellEntry[] {
	if (!Array.isArray(value)) return [];
	const entries: PlanCellEntry[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const record = item as Record<string, unknown>;
		const step = String(record.step ?? "").trim();
		if (!step) continue;
		const status = String(record.status ?? "pending").toLowerCase().replace(/[\s_-]/g, "");
		entries.push({
			step,
			status:
				status === "completed" || status === "complete" || status === "done"
					? "completed"
					: status === "inprogress" || status === "active" || status === "running"
						? "inProgress"
						: "pending",
		});
	}
	return entries;
}

type PlanCellExecutionStatus = "done" | "running" | "queued" | "error";

class PlanCellComponent implements Component {
	constructor(
		private readonly entries: PlanCellEntry[],
		private readonly executionStatus: PlanCellExecutionStatus,
		private readonly markdownText?: string,
		private readonly bodyText?: string,
	) {}

	render(width: number): string[] {
		const safeWidth = Math.max(1, width);
		const marker =
			this.executionStatus === "done"
				? theme.fg("success", "✓")
				: this.executionStatus === "error"
					? theme.fg("error", "✗")
					: this.executionStatus === "running"
						? theme.fg("bashMode", "●")
						: theme.fg("muted", "○");
		const countLabel = this.entries.length
			? ` · ${this.entries.length} step${this.entries.length === 1 ? "" : "s"}`
			: "";
		const header =
			`${marker} ${theme.fg("muted", "Plan")}` +
			`${theme.fg("dim", countLabel)}${theme.fg("dim", ` · ${this.executionStatus}`)}`;
		const lines = [truncateToWidth(` ${header}`, safeWidth, "")];

		if (this.executionStatus === "error") {
			this.addIndented(lines, this.bodyText || "Plan update failed", "  ", safeWidth, "error");
		} else if (this.markdownText) {
			this.addIndented(lines, this.markdownText, "  ", safeWidth, "toolOutput");
		} else if (this.entries.length > 0) {
			for (const entry of this.entries) {
				const itemMarker =
					entry.status === "completed"
						? theme.fg("success", "✓")
						: entry.status === "inProgress"
							? theme.fg("bashMode", "▶")
							: theme.fg("muted", "○");
				this.addIndented(lines, entry.step, `  ${itemMarker} `, safeWidth, "toolOutput");
			}
		} else {
			this.addIndented(lines, this.bodyText || "Waiting for plan steps…", "  ", safeWidth, "dim");
		}
		return lines;
	}

	invalidate(): void {}

	private addIndented(
		lines: string[],
		text: string,
		prefix: string,
		width: number,
		color: "toolOutput" | "error" | "dim",
	): void {
		const continuation = " ".repeat(visibleWidth(prefix));
		const availableWidth = Math.max(1, width - visibleWidth(prefix));
		for (const sourceLine of text.split("\n")) {
			const coloredLine = theme.fg(color, sourceLine || " ");
			const wrapped = wrapTextWithAnsi(coloredLine, availableWidth);
			for (const [index, line] of (wrapped.length > 0 ? wrapped : [""]).entries()) {
				lines.push(truncateToWidth(`${index === 0 ? prefix : continuation}${line}`, width, ""));
			}
		}
	}
}

export class ToolExecutionComponent extends Container {
	private contentPanel: ToolPanel;
	private selfRenderContainer: Container;
	private callRendererComponent?: Component;
	private resultRendererComponent?: Component;
	private ipythonCellComponent?: IPythonCellComponent;
	private rendererState: any = {};
	private imageComponents: Image[] = [];
	private toolName: string;
	private toolCallId: string;
	private args: any;
	private expanded = false;
	private editDiffsExpanded = false;
	private showExpandHint = true;
	private showImages: boolean;
	private includeImageDimensions: boolean;
	private isPartial = true;
	private toolDefinition?: ToolExecutionDefinition;
	private builtInToolDefinition?: ToolDefinition<any, any>;
	private ui: TUI;
	private cwd: string;
	private executionStarted = false;
	private argsComplete = false;
	private pendingSentAgentMessages: KernelSentAgentMessage[] = [];
	private result?: {
		content: Array<{ type: string; text?: string; data?: string; mimeType?: string }>;
		isError: boolean;
		details?: any;
	};
	private hideComponent = false;
	private shellCompletion?: ShellCompletion;
	private shellCompletionAmbiguous = false;
	private readonly resultListeners = new Set<() => void>();

	constructor(
		toolName: string,
		toolCallId: string,
		args: any,
		options: ToolExecutionOptions = {},
		toolDefinition: ToolExecutionDefinition | undefined,
		ui: TUI,
		cwd: string,
	) {
		super();
		this.toolName = toolName;
		this.toolCallId = toolCallId;
		this.args = args;
		this.toolDefinition = toolDefinition;
		this.builtInToolDefinition = createReplayBuiltInToolDefinition(toolName, cwd, toolDefinition);
		this.showImages = options.showImages ?? true;
		this.includeImageDimensions = options.includeImageDimensions ?? true;
		this.ui = ui;
		this.cwd = cwd;

		// Always create both shell variants. contentPanel is the tool panel used
		// for default renderer-based composition (and the generic fallback when no
		// tool definition exists). selfRenderContainer is used when the tool
		// renders its own framing.
		this.contentPanel = new ToolPanel();
		this.selfRenderContainer = new Container();

		if (this.toolName === "update_plan" || (this.hasRendererDefinition() && this.getRenderShell() === "self")) {
			this.addChild(this.selfRenderContainer);
		} else {
			this.addChild(this.contentPanel);
		}

		this.updateDisplay();
	}

	private getCallRenderer(): ToolDefinition<any, any>["renderCall"] | undefined {
		if (!this.builtInToolDefinition) {
			return this.toolDefinition?.renderCall;
		}
		if (!this.toolDefinition) {
			return this.builtInToolDefinition.renderCall;
		}
		return this.toolDefinition.renderCall ?? this.builtInToolDefinition.renderCall;
	}

	private getResultRenderer(): ToolDefinition<any, any>["renderResult"] | undefined {
		if (!this.builtInToolDefinition) {
			return this.toolDefinition?.renderResult;
		}
		if (!this.toolDefinition) {
			return this.builtInToolDefinition.renderResult;
		}
		return this.toolDefinition.renderResult ?? this.builtInToolDefinition.renderResult;
	}

	private hasRendererDefinition(): boolean {
		return this.builtInToolDefinition !== undefined || this.toolDefinition !== undefined;
	}

	private getRenderShell(): "default" | "self" {
		if (this.shouldUseIpythonRenderer()) {
			return "self";
		}
		if (!this.builtInToolDefinition) {
			return this.toolDefinition?.renderShell ?? "default";
		}
		if (!this.toolDefinition) {
			return this.builtInToolDefinition.renderShell ?? "default";
		}
		return this.toolDefinition.renderShell ?? this.builtInToolDefinition.renderShell ?? "default";
	}

	private shouldUseIpythonRenderer(): boolean {
		return this.toolName === "ipython" && !this.toolDefinition?.renderCall && !this.toolDefinition?.renderResult;
	}

	private isBuiltInEditTool(): boolean {
		return (
			this.toolName === "edit" &&
			(this.toolDefinition === undefined || this.toolDefinition.replayBuiltInToolName === "edit")
		);
	}

	private getRenderContext(lastComponent: Component | undefined): ToolRenderContext {
		return {
			args: this.args,
			toolCallId: this.toolCallId,
			invalidate: () => {
				this.invalidate();
				this.ui.requestRender();
			},
			lastComponent,
			state: this.rendererState,
			cwd: this.cwd,
			executionStarted: this.executionStarted,
			argsComplete: this.argsComplete,
			isPartial: this.isPartial,
			expanded: this.isBuiltInEditTool() ? this.editDiffsExpanded : this.expanded,
			showExpandHint: this.showExpandHint,
			showImages: this.showImages,
			includeImageDimensions: this.includeImageDimensions,
			isError: this.result?.isError ?? false,
		};
	}

	private createCallFallback(): Component {
		const summary = summarizeToolArgs(this.toolName, this.args);
		if (!summary) {
			return new Text(theme.fg("toolTitle", this.toolName), 0, 0);
		}
		return new Text(
			`${theme.fg("toolTitle", this.toolName)}${theme.fg("dim", " · ")}${theme.fg("dim", summary)}`,
			0,
			0,
		);
	}

	private createResultFallback(): Component | undefined {
		const output = this.getTextOutput();
		if (!output) {
			return undefined;
		}
		// Collapsed conversation mode: keep the tool row to a short preview so
		// hosted web_search / large dumps never paint the full payload inline.
		if (!this.expanded) {
			// web_search already carries the query on the panel header — skip the
			// body so collapsed mode stays a single status line (ipython-like).
			if (isWebSearchToolName(this.toolName)) {
				return undefined;
			}
			const preview = formatCollapsedToolPreview(output);
			if (!preview) return undefined;
			return new Text(theme.fg("toolOutput", preview), 0, 0);
		}
		return new Text(theme.fg("toolOutput", truncateExpandedToolOutput(output)), 0, 0);
	}

	updateArgs(args: any): void {
		this.args = args;
		this.updateDisplay();
	}

	/** Tool name used for recap / file-change attribution. */
	getToolName(): string {
		return this.toolName;
	}

	/** Latest args (needed for edit path when merging turn file changes). */
	getArgs(): any {
		return this.args;
	}

	markExecutionStarted(): void {
		this.executionStarted = true;
		this.updateDisplay();
		this.ui.requestRender();
	}

	setArgsComplete(): void {
		this.argsComplete = true;
		this.updateDisplay();
		this.ui.requestRender();
	}

	updateResult(
		result: {
			content: Array<{ type: string; text?: string; data?: string; mimeType?: string }>;
			details?: any;
			isError: boolean;
		},
		isPartial = false,
	): void {
		const details =
			typeof result.details === "object" && result.details !== null
				? (result.details as Record<string, unknown>)
				: {};
		const sentAgentMessages = Array.isArray(details.sentAgentMessages) ? [...details.sentAgentMessages] : [];
		for (const message of this.pendingSentAgentMessages) {
			if (
				!sentAgentMessages.some(
					(entry) => typeof entry === "object" && entry !== null && "id" in entry && entry.id === message.id,
				)
			) {
				sentAgentMessages.push(message);
			}
		}
		this.result = sentAgentMessages.length > 0 ? { ...result, details: { ...details, sentAgentMessages } } : result;
		this.isPartial = isPartial;
		this.updateDisplay();
		for (const listener of this.resultListeners) listener();
	}

	getBackgroundShellHandle(): BackgroundShellHandle | undefined {
		return this.shouldUseIpythonRenderer() && !this.isPartial && !this.result?.isError
			? readBackgroundShellHandle(getIpythonCodeFromArgs(this.args), this.result?.details)
			: undefined;
	}

	getAssignedShellCommand(): string | undefined {
		return this.shouldUseIpythonRenderer() && !this.isPartial && !this.result?.isError
			? readAssignedShellCommand(getIpythonCodeFromArgs(this.args), this.result?.details)
			: undefined;
	}

	hasRunningBackgroundShell(): boolean {
		const handle = this.getBackgroundShellHandle();
		return (
			handle !== undefined &&
			handle.exitCode === undefined &&
			this.shellCompletion === undefined &&
			!this.shellCompletionAmbiguous
		);
	}

	markShellCompletionAmbiguous(): void {
		if (!this.hasRunningBackgroundShell()) return;
		this.shellCompletionAmbiguous = true;
		this.updateDisplay();
		this.ui.requestRender();
	}

	onResultUpdate(listener: () => void): () => void {
		this.resultListeners.add(listener);
		return () => this.resultListeners.delete(listener);
	}

	isResultPending(): boolean {
		return this.isPartial;
	}

	attachShellCompletion(completion: ShellCompletion): boolean {
		if (this.shellCompletion) return false;
		this.shellCompletion = completion;
		this.shellCompletionAmbiguous = false;
		this.updateDisplay();
		this.ui.requestRender();
		return true;
	}

	appendSentAgentMessage(message: KernelSentAgentMessage): void {
		if (this.pendingSentAgentMessages.some((entry) => entry.id === message.id)) {
			return;
		}
		this.pendingSentAgentMessages.push(message);
		if (this.result) {
			this.updateResult(this.result, this.isPartial);
		}
	}

	setExpanded(expanded: boolean): void {
		this.expanded = expanded;
		this.updateDisplay();
	}

	setEditDiffsExpanded(expanded: boolean): void {
		if (this.editDiffsExpanded === expanded) {
			return;
		}
		this.editDiffsExpanded = expanded;
		this.updateDisplay();
	}

	setShowExpandHint(show: boolean): void {
		if (this.showExpandHint === show) {
			return;
		}
		this.showExpandHint = show;
		this.updateDisplay();
	}

	setShowImages(show: boolean): void {
		this.showImages = show;
		this.updateDisplay();
	}

	setIncludeImageDimensions(include: boolean): void {
		this.includeImageDimensions = include;
		this.updateDisplay();
	}

	override invalidate(): void {
		super.invalidate();
		this.updateDisplay();
	}

	override render(width: number): string[] {
		if (this.hideComponent) {
			return [];
		}
		// Refresh the animated glyph without rebuilding the whole panel, for as long
		// as panelStatus() is still animating (including partial streaming results).
		if (this.isStatusAnimating() && !this.usesSelfRenderShell()) {
			this.contentPanel.setHeader(this.panelHeader());
		}
		return super.render(width);
	}

	private isStatusAnimating(): boolean {
		if (!this.executionStarted) {
			return false;
		}
		// Matches panelStatus(): animating until a non-partial or error result lands.
		if (this.result && !this.isPartial) {
			return false;
		}
		return !this.result?.isError;
	}

	private usesSelfRenderShell(): boolean {
		return this.toolName === "update_plan" || (this.hasRendererDefinition() && this.getRenderShell() === "self");
	}

	private updateDisplay(): void {
		let hasContent = false;
		this.hideComponent = false;
		if (this.toolName === "update_plan") {
			this.selfRenderContainer.clear();
			this.selfRenderContainer.addChild(this.createPlanCellComponent());
			hasContent = true;
		} else if (this.hasRendererDefinition() && this.getRenderShell() === "self") {
			this.selfRenderContainer.clear();

			if (this.shouldUseIpythonRenderer()) {
				const state = {
					code: getIpythonCodeFromArgs(this.args),
					backgroundShell: this.getBackgroundShellHandle(),
					shellCompletion: this.shellCompletion,
					shellCompletionAmbiguous: this.shellCompletionAmbiguous,
					content: this.result?.content,
					details: this.result?.details,
					isPartial: this.isPartial,
					isError: this.result?.isError ?? false,
					expanded: this.expanded,
					editDiffsExpanded: this.editDiffsExpanded,
					executionStarted: this.executionStarted,
					argsComplete: this.argsComplete,
					showExpandHint: this.showExpandHint,
					showImages: this.showImages,
					cwd: this.cwd,
				};
				if (!this.ipythonCellComponent) {
					this.ipythonCellComponent = new IPythonCellComponent(state);
				} else {
					this.ipythonCellComponent.update(state);
				}
				this.selfRenderContainer.addChild(this.ipythonCellComponent);
				hasContent = true;
			} else {
				hasContent = this.mountRenderers(this.selfRenderContainer, true);
			}
		} else {
			// Default shell: tool panel with a `label · status` header so the block
			// is self-identifying. The header replaces the bold-tool-name fallback.
			this.contentPanel.setHeader(this.panelHeader());
			this.contentPanel.clear();
			if (this.hasRendererDefinition()) {
				this.mountRenderers(this.contentPanel, false);
			} else {
				const fallbackText = this.formatToolExecution();
				if (fallbackText) {
					this.contentPanel.addChild(new Text(fallbackText, 0, 0));
				}
			}
			hasContent = true;
		}

		for (const img of this.imageComponents) {
			this.removeChild(img);
		}
		this.imageComponents = [];

		if (this.result) {
			const imageBlocks = this.result.content.filter((c) => c.type === "image");
			for (let i = 0; i < imageBlocks.length; i++) {
				const img = imageBlocks[i];
				if (!this.showImages || !img.data || !img.mimeType) continue;

				const imageComponent = new Image(
					img.data,
					img.mimeType,
					{ fallbackColor: (s: string) => theme.fg("toolOutput", s) },
					{
						fallbackOnly: true,
						fallbackPrefix: "    ╰─ ",
					},
				);
				this.imageComponents.push(imageComponent);
				this.addChild(imageComponent);
			}
		}

		if (this.hasRendererDefinition() && !hasContent && this.imageComponents.length === 0) {
			this.hideComponent = true;
		}
	}

	/**
	 * Mount the call/result renderer components into the given shell container.
	 * `useFallbacks` keeps the bold-tool-name call fallback for self-rendering
	 * tools; the default panel shell already names the tool in its header.
	 */
	private mountRenderers(container: Container | ToolPanel, useFallbacks: boolean): boolean {
		let hasContent = false;

		const callRenderer = this.getCallRenderer();
		if (!callRenderer) {
			if (useFallbacks) {
				container.addChild(this.createCallFallback());
				hasContent = true;
			}
		} else {
			try {
				const component = callRenderer(this.args, theme, this.getRenderContext(this.callRendererComponent));
				this.callRendererComponent = component;
				container.addChild(component);
				hasContent = true;
			} catch {
				this.callRendererComponent = undefined;
				if (useFallbacks) {
					container.addChild(this.createCallFallback());
					hasContent = true;
				}
			}
		}

		if (this.result) {
			const resultRenderer = this.getResultRenderer();
			if (!resultRenderer) {
				const component = this.createResultFallback();
				if (component) {
					container.addChild(component);
					hasContent = true;
				}
			} else {
				try {
					const component = resultRenderer(
						{ content: this.result.content as any, details: this.result.details },
						{ expanded: this.expanded, isPartial: this.isPartial },
						theme,
						this.getRenderContext(this.resultRendererComponent),
					);
					this.resultRendererComponent = component;
					container.addChild(component);
					hasContent = true;
				} catch {
					this.resultRendererComponent = undefined;
					const component = this.createResultFallback();
					if (component) {
						container.addChild(component);
						hasContent = true;
					}
				}
			}
		}

		return hasContent;
	}

	private planEntries(): PlanCellEntry[] {
		const resultDetails = this.result?.details;
		const resultPlan =
			typeof resultDetails === "object" && resultDetails !== null
				? (resultDetails as Record<string, unknown>).plan
				: undefined;
		const argsPlan =
			typeof this.args === "object" && this.args !== null
				? (this.args as Record<string, unknown>).plan
				: undefined;
		return readPlanCellEntries(Array.isArray(resultPlan) ? resultPlan : argsPlan);
	}

	private createPlanCellComponent(): PlanCellComponent {
		const entries = this.planEntries();
		let executionStatus: PlanCellExecutionStatus;
		if (this.result && !this.isPartial) {
			executionStatus = this.result.isError ? "error" : "done";
		} else if (this.result?.isError) {
			executionStatus = "error";
		} else {
			executionStatus = this.executionStarted ? "running" : "queued";
		}
		const markdownText =
			entries.length === 1 && (entries[0]!.step.length > 200 || entries[0]!.step.includes("\n"))
				? entries[0]!.step
				: undefined;
		const output = this.getTextOutput();
		const bodyText =
			output ||
			(executionStatus === "error"
				? "Plan update failed"
				: executionStatus === "running"
					? "Waiting for plan steps…"
					: "Plan queued");
		return new PlanCellComponent(entries, executionStatus, markdownText, bodyText);
	}

	private panelHeader(): string {
		const label = this.toolDefinition?.label ?? this.builtInToolDefinition?.label ?? this.toolName;
		const argsSummary = summarizeToolArgs(this.toolName, this.args);
		const status = this.panelStatus();
		if (!argsSummary) {
			return `${theme.fg("muted", label)}${theme.fg("dim", " · ")}${status}`;
		}
		return `${theme.fg("muted", label)}${theme.fg("dim", " · ")}${theme.fg("dim", argsSummary)}${theme.fg("dim", " · ")}${status}`;
	}

	private panelStatus(): string {
		if (this.result && !this.isPartial) {
			return this.result.isError ? theme.fg("error", "error") : theme.fg("success", "done");
		}
		if (this.result?.isError) {
			return theme.fg("error", "error");
		}
		if (this.executionStarted) {
			return theme.fg("bashMode", `${workingIconFrame(getWorkingPulseFrame())} running`);
		}
		return theme.fg("muted", "queued");
	}

	private getTextOutput(): string {
		return getRenderedTextOutput(this.result, this.showImages, {
			includeImageDimensions: this.includeImageDimensions,
		});
	}

	private formatFallbackPreview(text: string): string {
		if (this.expanded) {
			return truncateExpandedToolOutput(text);
		}
		return formatCollapsedToolPreview(text);
	}

	private formatToolExecution(): string {
		const parts: string[] = [];
		const argsSummary = summarizeToolArgs(this.toolName, this.args);
		if (argsSummary) {
			// Header already shows args for the panel shell; avoid duplicating the
			// query/command in the body when collapsed.
			if (this.expanded) {
				parts.push(theme.fg("dim", argsSummary));
			}
		} else {
			const content = JSON.stringify(this.args, null, 2);
			if (content && content !== "{}") {
				parts.push(this.formatFallbackPreview(content));
			}
		}
		const output = this.getTextOutput();
		if (output) {
			if (!this.expanded && isWebSearchToolName(this.toolName)) {
				// Collapsed web_search: header-only (query · done). Expand for hits.
			} else {
				parts.push(this.formatFallbackPreview(output));
			}
		}
		return parts.join("\n\n");
	}
}

const COLLAPSED_TOOL_PREVIEW_CHARS = 240;
const COLLAPSED_TOOL_PREVIEW_LINES = 2;
const EXPANDED_TOOL_PREVIEW_CHARS = 12_000;
const EXPANDED_TOOL_PREVIEW_LINES = 120;

function isWebSearchToolName(toolName: string): boolean {
	const name = toolName.toLowerCase();
	return name === "web_search" || name === "websearch" || name === "web-search";
}

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
		const query = pick("query", "q", "search", "text");
		return query ? truncatePlain(query, 80) : "";
	}
	if (name === "web_fetch" || name === "webfetch" || name === "web-fetch" || name === "fetch_url") {
		const url = pick("url", "uri", "href");
		return url ? truncatePlain(url, 80) : "";
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

function truncateExpandedToolOutput(text: string): string {
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

export function selectLatestToolExpandHint(
	existingComponents: readonly Component[],
	latestComponent: ToolExecutionComponent,
): void {
	for (let index = existingComponents.length - 1; index >= 0; index--) {
		const component = existingComponents[index];
		if (component instanceof ToolExecutionComponent) {
			component.setShowExpandHint(false);
			break;
		}
	}
	latestComponent.setShowExpandHint(true);
}
