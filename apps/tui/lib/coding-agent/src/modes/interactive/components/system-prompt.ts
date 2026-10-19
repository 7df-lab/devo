import { Container, Spacer, Text } from "@earendil-works/pi-tui";
import { theme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";
import { truncateToVisualLinesHead } from "./visual-truncate.js";

const PREVIEW_VISUAL_LINES = 18;

/**
 * `/system-prompt` dump: bordered chrome with a short head preview by default.
 * Full text expands via Ctrl+O (`setExpanded`), same path as bash/tool output.
 */
export class SystemPromptComponent extends Container {
	private readonly prompt: string;
	private readonly lineCount: number;
	private readonly charCount: number;
	private expanded: boolean;
	private contentContainer: Container;

	constructor(prompt: string, expanded = false) {
		super();
		this.prompt = prompt.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
		this.charCount = prompt.length;
		this.lineCount = this.prompt.length === 0 ? 0 : this.prompt.split("\n").length;
		this.expanded = expanded;

		const borderColor = (str: string) => theme.fg("dim", str);
		this.addChild(new Spacer(1));
		this.addChild(new DynamicBorder(borderColor));
		this.contentContainer = new Container();
		this.addChild(this.contentContainer);
		this.addChild(new DynamicBorder(borderColor));
		this.updateDisplay();
	}

	setExpanded(expanded: boolean): void {
		if (this.expanded === expanded) return;
		this.expanded = expanded;
		this.updateDisplay();
	}

	override invalidate(): void {
		super.invalidate();
		this.updateDisplay();
	}

	private updateDisplay(): void {
		this.contentContainer.clear();

		const lineLabel = this.lineCount === 1 ? "1 line" : `${this.lineCount} lines`;
		const header = `${theme.fg("accent", "System Prompt")} ${theme.fg(
			"dim",
			`(${this.charCount.toLocaleString()} chars · ${lineLabel})`,
		)}`;
		this.contentContainer.addChild(new Text(header, 1, 0));

		if (this.prompt.length === 0) {
			this.contentContainer.addChild(new Text(theme.fg("muted", "(empty)"), 1, 0));
			return;
		}

		const styled = this.prompt
			.split("\n")
			.map((line) => theme.fg("muted", line))
			.join("\n");

		if (this.expanded) {
			this.contentContainer.addChild(new Text(styled, 1, 0));
			return;
		}

		let cachedWidth: number | undefined;
		let cachedLines: string[] | undefined;
		let cachedSkipped = 0;
		this.contentContainer.addChild({
			render: (width: number) => {
				if (cachedLines === undefined || cachedWidth !== width) {
					const result = truncateToVisualLinesHead(styled, PREVIEW_VISUAL_LINES, width, 1);
					cachedLines = result.visualLines;
					cachedSkipped = result.skippedCount;
					cachedWidth = width;
				}
				const lines = [...(cachedLines ?? [])];
				if (cachedSkipped > 0) {
					lines.push(theme.fg("muted", ` ... ${cachedSkipped} more lines (Ctrl+O to expand)`));
				}
				return lines;
			},
			invalidate: () => {
				cachedWidth = undefined;
				cachedLines = undefined;
				cachedSkipped = 0;
			},
		});
	}
}
