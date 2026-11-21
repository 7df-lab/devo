import { test } from "node:test";
import assert from "node:assert/strict";
import { WebSearchSelectorComponent } from "./web-search-selector.js";
import { initTheme } from "../theme/theme.js";
import type { AgentConnectionWebSearchChoice } from "../../agent-connection/types.js";

initTheme("dark");

const choices: AgentConnectionWebSearchChoice[] = [
	{ label: "Follow defaults", choice: { kind: "inherit" } },
	{ label: "Model provider", choice: { kind: "provider" } },
	{ label: "Off", choice: { kind: "off" } },
	{ label: "Local search", choice: { kind: "mcp", serverId: "local", toolName: "search" }, note: "Not ready yet" },
];
const status = { search: { backend: "provider" }, fetch: { backend: "native" } };

test("web search selector selects built-in and custom choices, including not-ready entries", () => {
	let selected: unknown;
	const selector = new WebSearchSelectorComponent(
		{ kind: "inherit" }, choices, status, (choice) => { selected = choice; }, () => assert.fail("unexpected cancel"),
	);
	selector.getSelectList().handleInput("\x1b[A"); // wrap from the first entry to user MCP
	selector.getSelectList().handleInput("\r");
	assert.deepEqual(selected, { kind: "mcp", serverId: "local", toolName: "search" });

	const inherited = new WebSearchSelectorComponent(
		undefined, choices, status, (choice) => { selected = choice; }, () => assert.fail("unexpected cancel"),
	);
	inherited.getSelectList().handleInput("\x1b[B");
	inherited.getSelectList().handleInput("\x1b[B");
	inherited.getSelectList().handleInput("\r");
	assert.deepEqual(selected, { kind: "off" });
});

test("cancelling web search selector does not select a value", () => {
	let selected = false;
	let cancelled = false;
	const selector = new WebSearchSelectorComponent(
		{ kind: "provider" }, choices, status, () => { selected = true; }, () => { cancelled = true; },
	);
	selector.getSelectList().handleInput("\x1b");
	assert.equal(cancelled, true);
	assert.equal(selected, false);
});
