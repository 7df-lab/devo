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
	assert.equal(selected, undefined, "moving through the picker does not select anything");
	selector.handleInput("\x1b[D");
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
	assert.match(text(selector), /Enter add key/);
	assert.doesNotMatch(text(selector), /\/login/);
	assert.match(text(selector), /  ✓ Exa.*Key saved/);
});

test("configured backend is previewed only for an explicit saved override", () => {
	const explicit = new WebSearchSelectorComponent(choices[0]?.choice, choices, { ...status, searchChoice: choices[0]?.choice }, () => {}, () => {});
	assert.match(text(explicit), /Use configured backend.*serper/);
	const inherited = new WebSearchSelectorComponent({ kind: "inherit" }, choices, status, () => {}, () => {});
	assert.doesNotMatch(text(inherited), /Use configured backend/);
});

test("Left restores focus on the parent row, and Escape closes from either level", () => {
	for (const escapeFromSubmenu of [false, true]) {
		let closed = false;
		const selector = new WebSearchSelectorComponent(undefined, choices, status, () => {}, () => { closed = true; });
		selector.handleInput("\x1b[B");
		selector.handleInput("\r");
		assert.match(text(selector), /← back.*Esc close/);
		if (!escapeFromSubmenu) {
			selector.handleInput("\x1b[D");
			assert.match(text(selector), /›\s+Built-in.+›/);
			assert.doesNotMatch(text(selector), /Key required/);
			assert.equal(closed, false);
		}
		selector.handleInput("\x1b");
		assert.equal(closed, true);
	}
});

test("Enter on a missing key opens a masked field and selects only after the saved status returns", async () => {
	const calls: unknown[] = [];
	const updated = { ...status, searchChoices: choices.map((choice, index) => index === 1 ? { ...choice, keyStatus: "saved" as const } : choice) };
	const selector = new WebSearchSelectorComponent(undefined, choices, status,
		(choice) => { calls.push(choice); }, () => assert.fail("unexpected close"), assert.fail, {
			save: async (id, key) => { calls.push({ id, key }); return updated; },
			requestRender: () => {},
		});
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	selector.handleInput("\r");
	assert.match(text(selector), /Serper API key/);
	selector.handleInput("\x1b[D");
	assert.match(text(selector), /›\s+Serper.*Key required/);
	selector.handleInput("\r");
	selector.handleInput("\r");
	assert.match(text(selector), /Enter a single API key/);
	selector.handleInput("\x1b[200~test-placeholder\x1b[201~");
	assert.match(text(selector), /••••/);
	assert.doesNotMatch(text(selector), /test-placeholder/);
	assert.deepEqual(calls, []);
	selector.handleInput("\r");
	await new Promise<void>((resolve) => setImmediate(resolve));
	assert.deepEqual(calls, [{ id: "serper", key: "test-placeholder" }, choices[1]?.choice]);
	assert.doesNotMatch(text(selector), /test-placeholder/);
});

test("Escape from key entry discards unsubmitted input, and save errors never expose it", async () => {
	let closed = false;
	let saves = 0;
	const selector = new WebSearchSelectorComponent(undefined, choices, status,
		() => assert.fail("unexpected selection"), () => { closed = true; }, assert.fail, {
			save: async (_id, key) => { saves++; throw new Error(key); },
			requestRender: () => {},
		});
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	selector.handleInput("\r");
	selector.handleInput("test-placeholder");
	selector.handleInput("\x1b");
	assert.equal(saves, 0);
	assert.equal(closed, true);

	selector.handleInput("\r");
	selector.handleInput("test-placeholder");
	selector.handleInput("\r");
	await new Promise<void>((resolve) => setImmediate(resolve));
	assert.equal(saves, 1);
	assert.match(text(selector), /Couldn’t save the key/);
	assert.doesNotMatch(text(selector), /test-placeholder/);
});

test("unknown provider probes do not disable a choice and rejected keys can be replaced", () => {
	let selected = false;
	const pendingChoices = [
		{ label: "Model provider", choice: { kind: "provider" } as const, note: "Checking…" },
		{ label: "Exa", choice: { kind: "unified", backend: { kind: "mcp", serverId: "devo:builtin:exa", toolName: "search" } } as const, group: "builtin" as const, keyStatus: "rejected" as const },
	];
	const pendingStatus: AgentConnectionWebStatus = { ...status, searchChoices: pendingChoices, searchChoice: { kind: "inherit" } };
	const selector = new WebSearchSelectorComponent(undefined, pendingChoices, pendingStatus,
		() => { selected = true; }, () => {}, assert.fail, {
			save: async () => assert.fail("should not have saved yet"),
			requestRender: () => {},
		});
	selector.handleInput("\r");
	assert.equal(selected, true);
	assert.match(text(selector), /Checking/);
	selector.handleInput("\x1b[B");
	selector.handleInput("\r");
	assert.match(text(selector), /Key rejected/);
	assert.match(text(selector), /Enter replace key/);
	selector.handleInput("\r");
	assert.match(text(selector), /Exa API key/);
});
