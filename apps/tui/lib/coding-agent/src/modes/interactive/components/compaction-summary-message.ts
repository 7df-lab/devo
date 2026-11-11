import { Markdown, type MarkdownTheme, Spacer, Text } from "@earendil-works/pi-tui";
import type { CompactionSummaryMessage } from "../../../core/messages.js";
import { getMarkdownTheme, theme } from "../theme/theme.js";
import { ExpandableCustomMessageBox } from "./expandable-custom-message.js";

/** Compact context outcome with the full markdown summary available on demand. */
export class CompactionSummaryMessageComponent extends ExpandableCustomMessageBox {
	constructor(
		private readonly message: CompactionSummaryMessage,
		private readonly markdownTheme: MarkdownTheme = getMarkdownTheme(),
	) {
		super();
		this.updateDisplay();
	}

	protected updateDisplay(): void {
		this.clear();
		this.addChild(new Text(theme.fg("refinementHeader", "◆ Compaction · completed"), 0, 0));
		const before = this.message.tokensBeforeMeasured === false ? "unknown" : this.message.tokensBefore.toLocaleString();
		const after = this.message.tokensAfter?.toLocaleString();
		this.addChild(new Text(theme.fg("dim", after !== undefined ? `Context: ${before} → ${after} tokens` : `Context before: ${before} tokens`), 0, 0));
		this.addChild(new Spacer(1));
		const summary = this.message.summary.trim()
			? this.message.summary
			: "No summary was recorded for this compaction.";
		if (!this.expanded) {
			const preview = summary.replace(/\s+/g, " ").trim();
			this.addChild(new Text(theme.fg("refinementSummary", preview.length > 180 ? `${preview.slice(0, 180)}…` : preview), 0, 0));
			return;
		}

		this.addChild(
			new Markdown(summary, 0, 0, this.markdownTheme, {
				color: (text: string) => theme.fg("refinementSummary", text),
			}),
		);

		const instructions = this.message.customInstructions;
		const focus = instructions ? ` · focus: ${instructions}` : "";
		this.addChild(new Spacer(1));
		this.addChild(new Text(theme.fg("dim", `Compaction summary${focus}`), 1, 0));
	}
}
