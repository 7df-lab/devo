/**
 * Bug (live r25): typed text while a modal selector (approval dialog) was
 * open buffered into the composer, but Enter stayed bound to the dialog's
 * confirm — so a user typing a chat message during an approval dialog and
 * pressing Enter silently approved the gated command, leaving the message
 * as an unsent draft. The selector now consults onConfirmIntent first:
 * when a higher intent (mid-composition) consumes the Enter, the dialog
 * must not confirm.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { ExtensionSelectorComponent } from "../lib/coding-agent/src/modes/interactive/components/extension-selector.js";

initTheme("dark", true);

test("selector confirm consults onConfirmIntent before selecting", () => {
	const selections: string[] = [];
	const intents: boolean[] = [];
	const selector = new ExtensionSelectorComponent(
		"Approve command/request?",
		["Yes, proceed", "No, continue without running it"],
		(option) => selections.push(option),
		() => {},
		{
			onConfirmIntent: () => intents.pop() ?? false,
		},
	);

	// No higher intent: Enter confirms the highlighted option as before.
	intents.push(false);
	selector.handleInput("\r");
	assert.deepEqual(selections, ["Yes, proceed"]);

	// Higher intent active (user mid-composition): Enter is consumed by the
	// intent and the dialog must NOT confirm.
	intents.push(true);
	selector.handleInput("\r");
	assert.deepEqual(selections, ["Yes, proceed"]);

	// A "\n" Enter behaves the same way.
	intents.push(true);
	selector.handleInput("\n");
	assert.deepEqual(selections, ["Yes, proceed"]);

	intents.push(false);
	selector.handleInput("\n");
	assert.deepEqual(selections, ["Yes, proceed", "Yes, proceed"]);
});

test("selector without onConfirmIntent confirms directly", () => {
	const selections: string[] = [];
	const selector = new ExtensionSelectorComponent("Pick", ["a", "b"], (option) => selections.push(option), () => {});
	selector.handleInput("\r");
	assert.deepEqual(selections, ["a"]);
});
