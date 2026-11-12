import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import { Container, type SelectItem, SelectList, type SelectListLayoutOptions, Text } from "@earendil-works/pi-tui";
import { getSelectListTheme, theme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";
import { keyHint, keyText } from "./keybinding-hints.js";

const THINKING_SELECT_LIST_LAYOUT: SelectListLayoutOptions = {
	minPrimaryColumnWidth: 12,
	maxPrimaryColumnWidth: 32,
};

const LEVEL_DESCRIPTIONS: Record<ThinkingLevel, string> = {
	off: "No reasoning",
	minimal: "Very brief reasoning",
	low: "Light reasoning",
	medium: "Moderate reasoning",
	high: "Deep reasoning",
	xhigh: "Very deep reasoning",
	max: "Maximum reasoning",
};

export class ThinkingSelectorComponent extends Container {
	private selectList: SelectList;

	constructor(
		currentLevel: ThinkingLevel,
		availableLevels: ThinkingLevel[],
		onSelect: (level: ThinkingLevel) => void,
		onCancel: () => void,
	) {
		super();

		const thinkingLevels: SelectItem[] = availableLevels.map((level) => ({
			value: level,
			label: level,
			description: LEVEL_DESCRIPTIONS[level],
		}));

		this.addChild(new DynamicBorder());
		this.addChild(new Text(theme.fg("accent", "Reasoning effort"), 1, 0));

		this.selectList = new SelectList(
			thinkingLevels,
			thinkingLevels.length,
			getSelectListTheme(),
			THINKING_SELECT_LIST_LAYOUT,
		);

		const currentIndex = thinkingLevels.findIndex((item) => item.value === currentLevel);
		if (currentIndex !== -1) {
			this.selectList.setSelectedIndex(currentIndex);
		}

		this.selectList.onSelect = (item) => {
			onSelect(item.value as ThinkingLevel);
		};

		this.selectList.onCancel = () => {
			onCancel();
		};

		this.addChild(this.selectList);
		this.addChild(new Text(
			theme.fg("dim", `${keyText("tui.select.up", { primaryOnly: true })}/${keyText("tui.select.down", { primaryOnly: true })}`)
				+ theme.fg("muted", " move  ")
				+ keyHint("tui.select.confirm", "select", { primaryOnly: true }) + "  "
				+ keyHint("tui.select.cancel", "cancel", { primaryOnly: true }),
			1, 0,
		));

		this.addChild(new DynamicBorder());
	}

	getSelectList(): SelectList {
		return this.selectList;
	}
}
