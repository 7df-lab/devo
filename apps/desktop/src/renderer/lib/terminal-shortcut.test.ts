import { describe, expect, test } from "bun:test";
import { isTerminalToggleShortcut } from "./terminal-shortcut";

describe("isTerminalToggleShortcut", () => {
	test("matches Cmd+J and Ctrl+J without matching plain J or Alt+J", () => {
		const cases = [
			isTerminalToggleShortcut({
				key: "j",
				metaKey: true,
				ctrlKey: false,
				altKey: false,
			}),
			isTerminalToggleShortcut({
				key: "J",
				metaKey: false,
				ctrlKey: true,
				altKey: false,
			}),
			isTerminalToggleShortcut({
				key: "j",
				metaKey: false,
				ctrlKey: false,
				altKey: false,
			}),
			isTerminalToggleShortcut({
				key: "j",
				metaKey: false,
				ctrlKey: false,
				altKey: true,
			}),
		];

		expect(cases).toEqual([true, true, false, false]);
	});
});
