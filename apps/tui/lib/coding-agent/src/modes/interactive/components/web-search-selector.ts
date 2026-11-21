import { Container, type SelectItem, SelectList, Text } from "@earendil-works/pi-tui";
import stripAnsi from "strip-ansi";
import type { AgentConnectionWebSearchChoice, AgentConnectionWebStatus, WebSearchChoice } from "../../agent-connection/types.js";
import { getSelectListTheme, theme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";
import { keyHint } from "./keybinding-hints.js";

function sameChoice(left: WebSearchChoice | undefined, right: WebSearchChoice): boolean {
	return left?.kind === right.kind
		&& (left?.kind !== "mcp" || (right.kind === "mcp" && left.serverId === right.serverId && left.toolName === right.toolName));
}

function singleLine(text: string): string {
	return stripAnsi(text).replace(/[\x00-\x1f\x7f-\x9f]/g, " ");
}

export class WebSearchSelectorComponent extends Container {
	private readonly selectList: SelectList;

	constructor(
		currentChoice: WebSearchChoice | undefined,
		choices: AgentConnectionWebSearchChoice[],
		status: Pick<AgentConnectionWebStatus, "search" | "fetch">,
		onSelect: (choice: WebSearchChoice) => void,
		onCancel: () => void,
	) {
		super();
		const items: SelectItem[] = choices.map((entry, index) => ({
			value: String(index),
			label: singleLine(entry.label),
			description: entry.note && singleLine(entry.note),
		}));

		this.addChild(new DynamicBorder());
		this.addChild(new Text(theme.fg("accent", "Web search"), 1, 0));
		this.addChild(new Text(
			`Search: ${singleLine(status.search.backend)}${status.search.note ? ` — ${singleLine(status.search.note)}` : ""}\nFetch (read-only): ${singleLine(status.fetch.backend)}${status.fetch.note ? ` — ${singleLine(status.fetch.note)}` : ""}`,
			1,
			0,
		));
		this.selectList = new SelectList(items, Math.min(items.length, 10), getSelectListTheme());
		const currentIndex = choices.findIndex(({ choice }) => sameChoice(currentChoice, choice));
		if (currentIndex !== -1) this.selectList.setSelectedIndex(currentIndex);
		this.selectList.onSelect = (item) => {
			const choice = choices[Number(item.value)]?.choice;
			if (choice) onSelect(choice);
		};
		this.selectList.onCancel = onCancel;
		this.addChild(this.selectList);
		this.addChild(new Text(
			keyHint("tui.select.confirm", "select", { primaryOnly: true })
				+ "  " + keyHint("tui.select.cancel", "cancel", { primaryOnly: true }),
			1,
			0,
		));
		this.addChild(new DynamicBorder());
	}

	getSelectList(): SelectList {
		return this.selectList;
	}
}
