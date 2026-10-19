import { Text } from "@earendil-works/pi-tui";

interface Expandable {
	setExpanded(expanded: boolean): void;
}

interface EditDiffsExpandable {
	setEditDiffsExpanded(expanded: boolean): void;
}

export function isExpandable(obj: unknown): obj is Expandable {
	return typeof obj === "object" && obj !== null && "setExpanded" in obj && typeof obj.setExpanded === "function";
}

export function hasEditDiffsExpansion(obj: unknown): obj is EditDiffsExpandable {
	return (
		typeof obj === "object" &&
		obj !== null &&
		"setEditDiffsExpanded" in obj &&
		typeof (obj as EditDiffsExpandable).setEditDiffsExpanded === "function"
	);
}

export class ExpandableText extends Text implements Expandable {
	constructor(
		private readonly getCollapsedText: () => string,
		private readonly getExpandedText: () => string,
		expanded = false,
		paddingX = 0,
		paddingY = 0,
	) {
		super(expanded ? getExpandedText() : getCollapsedText(), paddingX, paddingY);
	}

	setExpanded(expanded: boolean): void {
		this.setText(expanded ? this.getExpandedText() : this.getCollapsedText());
	}
}
