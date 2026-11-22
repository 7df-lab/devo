import { test } from "node:test";
import assert from "node:assert/strict";
import stripAnsi from "strip-ansi";
import { WebSearchSelectorComponent } from "./web-search-selector.js";
import { initTheme } from "../theme/theme.js";
import type { AgentConnectionWebSearchChoice, AgentConnectionWebStatus } from "../../agent-connection/types.js";

initTheme("dark");

const choices: AgentConnectionWebSearchChoice[] = [
	{ label: "Model provider", choice: { kind: "unified", backend: { kind: "provider" } }, note: "Automatic · hosted search enabled" },
	{ label: "Serper", choice: { kind: "unified", backend: { kind: "mcp", serverId: "devo:builtin:serper", toolName: "search" } }, group: "builtin", keyStatus: "required" },
	{ label: "Exa", choice: { kind: "unified", backend: { kind: "mcp", serverId: "devo:builtin:exa", toolName: "search" } }, group: "builtin", keyStatus: "saved" },
	{ label: "Local search", choice: { kind: "unified", backend: { kind: "mcp", serverId: "custom-search", toolName: "search" } }, group: "custom" },
	{ label: "Off", choice: { kind: "unified", backend: { kind: "off" } } },
	{ label: "Follow defaults", choice: { kind: "inherit" } },
];
const status: AgentConnectionWebStatus = {
	search: { backend: "serper" },
	fetch: { backend: "native" },
	searchChoice: { kind: "inherit" },
	searchChoices: choices,
	configuredBackend: "serper",
};

function text(selector: WebSearchSelectorComponent): string {
	return stripAnsi(selector.render(100).join("\n"));
}

test("web picker offers provider and nested built-in/custom menus with key status", () => {
	let selected: unknown;
	const selector = new WebSearchSelectorComponent(undefined, choices, status, (choice) => { selected = choice; }, () => assert.fail("unexpected cancel"));
	assert.match(text(selector), /Model provider/);
	assert.match(text(selector), /Built-in/);
	assert.match(text(selector), /My MCP tools/);
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	assert.match(text(selector), /Serper.*Key required/);
	assert.match(text(selector), /Exa.*Key saved/);
	selector.handleInput("\r");
	assert.equal(selected, undefined, "required key must not be saved");
	selector.handleInput("\x1b");
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	assert.match(text(selector), /Local search/);
	selector.handleInput("\r");
	assert.deepEqual(selected, choices[3]?.choice);
});

test("web picker marks the active selection separately from keyboard focus", () => {
	const activeStatus = { ...status, searchChoice: choices[0]?.choice, activeChoice: choices[0]?.choice };
	const selector = new WebSearchSelectorComponent(choices[0]?.choice, choices, activeStatus, () => {}, () => {});
	const lines = text(selector).split("\n");
	assert.match(lines[0] ?? "", /›.*✓.*Model provider/);
	assert.doesNotMatch(text(selector), /Current:|Web search|──/);
});

test("the active supplier is visible on both levels without repeating the group name", () => {
	const active = choices[2]?.choice;
	const activeStatus = { ...status, searchChoice: active, activeChoice: active };
	const selector = new WebSearchSelectorComponent(active, choices, activeStatus, () => {}, () => {});
	assert.match(text(selector), /› ✓ Built-in · Exa\s+›/);
	selector.handleInput("\r");
	assert.match(text(selector), /› ✓ Exa.*Key saved/);
	selector.handleInput("\x1b[A");
	assert.match(text(selector), /Save a Serper key in \/login to use it\./);
	assert.match(text(selector), /  ✓ Exa.*Key saved/);
});

test("configured backend is previewed only for an explicit saved override", () => {
	const explicit = new WebSearchSelectorComponent(choices[0]?.choice, choices, { ...status, searchChoice: choices[0]?.choice }, () => {}, () => {});
	assert.match(text(explicit), /Use configured backend.*serper/);
	const inherited = new WebSearchSelectorComponent({ kind: "inherit" }, choices, status, () => {}, () => {});
	assert.doesNotMatch(text(inherited), /Use configured backend/);
});

test("escape returns from a submenu, then closes the picker", () => {
	let cancelled = false;
	const selector = new WebSearchSelectorComponent(undefined, choices, status, () => {}, () => { cancelled = true; });
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	selector.handleInput("\x1b");
	assert.match(text(selector), /Built-in/);
	assert.equal(cancelled, false);
	selector.handleInput("\x1b");
	assert.equal(cancelled, true);
});

test("unknown provider probes do not disable a choice and rejected keys are surfaced", () => {
	let selected = false;
	const pendingChoices = [
		{ label: "Model provider", choice: { kind: "provider" } as const, note: "Checking…" },
		{ label: "Exa", choice: { kind: "unified", backend: { kind: "mcp", serverId: "devo:builtin:exa", toolName: "search" } } as const, group: "builtin" as const, keyStatus: "rejected" as const },
	];
	const pendingStatus: AgentConnectionWebStatus = { ...status, searchChoices: pendingChoices, searchChoice: { kind: "inherit" } };
	const selector = new WebSearchSelectorComponent(undefined, pendingChoices, pendingStatus, () => { selected = true; }, () => {});
	selector.handleInput("\r");
	assert.equal(selected, true);
	assert.match(text(selector), /Checking/);
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	assert.match(text(selector), /Key rejected/);
});
