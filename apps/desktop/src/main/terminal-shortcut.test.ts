import { describe, expect, test } from "bun:test";
import { isTerminalToggleInput } from "./terminal-shortcut";

describe("isTerminalToggleInput", () => {
	test("matches native Cmd+J and Ctrl+J keydown events", () => {
		const cases = [
			isTerminalToggleInput({
				key: "j",
				meta: true,
				control: false,
				alt: false,
				type: "keyDown",
			}),
			isTerminalToggleInput({
				key: "J",
				meta: false,
				control: true,
				alt: false,
				type: "keyDown",
			}),
			isTerminalToggleInput({
				key: "j",
				meta: true,
				control: false,
				alt: false,
				type: "keyUp",
			}),
			isTerminalToggleInput({
				key: "j",
				meta: false,
				control: false,
				alt: false,
				type: "keyDown",
			}),
			isTerminalToggleInput({
				key: "j",
				meta: false,
				control: true,
				alt: true,
				type: "keyDown",
			}),
		];

		expect(cases).toEqual([true, true, false, false, false]);
	});
});
