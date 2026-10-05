import { Container, type SelectItem, SelectList } from "@earendil-works/pi-tui";
import { getSelectListTheme } from "../theme/theme.js";
import { DynamicBorder } from "./dynamic-border.js";

export type PermissionProfile = "default" | "autoReview" | "fullAccess";

const PROFILE_DESCRIPTIONS: Record<PermissionProfile, string> = {
	default: "Ask before risky actions",
	autoReview: "Auto-approve tools, review afterwards",
	fullAccess: "Allow everything without asking",
};

export const PERMISSION_PROFILES: PermissionProfile[] = ["default", "autoReview", "fullAccess"];

export class PermissionSelectorComponent extends Container {
	private selectList: SelectList;

	constructor(
		currentProfile: PermissionProfile,
		onSelect: (profile: PermissionProfile) => void,
		onCancel: () => void,
	) {
		super();

		const profiles: SelectItem[] = PERMISSION_PROFILES.map((profile) => ({
			value: profile,
			label: profile,
			description: PROFILE_DESCRIPTIONS[profile],
		}));

		this.addChild(new DynamicBorder());

		this.selectList = new SelectList(profiles, profiles.length, getSelectListTheme());

		const currentIndex = profiles.findIndex((item) => item.value === currentProfile);
		if (currentIndex !== -1) {
			this.selectList.setSelectedIndex(currentIndex);
		}

		this.selectList.onSelect = (item) => {
			onSelect(item.value as PermissionProfile);
		};

		this.selectList.onCancel = () => {
			onCancel();
		};

		this.addChild(this.selectList);

		this.addChild(new DynamicBorder());
	}

	getSelectList(): SelectList {
		return this.selectList;
	}
}
