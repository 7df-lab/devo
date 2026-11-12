import { Container, type SelectItem, SelectList, type SelectListLayoutOptions, Text } from "@earendil-works/pi-tui";
import { getAvailableThemes, getSelectListTheme, theme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";
import { keyHint, keyText } from "./keybinding-hints.js";

const THEME_SELECT_LIST_LAYOUT: SelectListLayoutOptions = {
	minPrimaryColumnWidth: 12,
	maxPrimaryColumnWidth: 32,
};

export class ThemeSelectorComponent extends Container {
	private selectList: SelectList;
	private title = new Text("", 1, 0);
	private help = new Text("", 1, 0);
	private onPreview: (themeName: string) => void;

	constructor(
		currentTheme: string,
		onSelect: (themeName: string) => void,
		onCancel: () => void,
		onPreview: (themeName: string) => void,
	) {
		super();
		this.onPreview = onPreview;

		const themes = getAvailableThemes();
		const themeItems: SelectItem[] = themes.map((name) => ({
			value: name,
			label: name,
			description: name === currentTheme ? "(current)" : undefined,
		}));

		this.addChild(new DynamicBorder());
		this.addChild(this.title);

		this.selectList = new SelectList(themeItems, 10, getSelectListTheme(), THEME_SELECT_LIST_LAYOUT);

		const currentIndex = themes.indexOf(currentTheme);
		if (currentIndex !== -1) {
			this.selectList.setSelectedIndex(currentIndex);
		}

		this.selectList.onSelect = (item) => {
			onSelect(item.value);
		};

		this.selectList.onCancel = () => {
			onCancel();
		};

		this.selectList.onSelectionChange = (item) => {
			this.onPreview(item.value);
		};

		this.addChild(this.selectList);
		this.addChild(this.help);

		this.addChild(new DynamicBorder());
	}

	render(width: number): string[] {
		// Theme changes while this selector is open, so resolve colors at render time.
		this.title.setText(theme.fg("accent", "Theme preview"));
		this.help.setText(
			theme.fg("dim", `${keyText("tui.select.up", { primaryOnly: true })}/${keyText("tui.select.down", { primaryOnly: true })}`)
				+ theme.fg("muted", " preview  ")
				+ keyHint("tui.select.confirm", "save", { primaryOnly: true }) + "  "
				+ keyHint("tui.select.cancel", "restore", { primaryOnly: true }),
		);
		return super.render(width);
	}

	getSelectList(): SelectList {
		return this.selectList;
	}
}
