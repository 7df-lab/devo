import assert from "node:assert/strict";
import { test } from "node:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import { initTheme } from "../theme/theme.js";
import { SubagentSummaryLine } from "./subagent-summary-line.js";

const plain = (lines: string[]) => lines.join("\n").replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");

function dock() {
	initTheme();
	const component = new SubagentSummaryLine();
	component.setScheduledCounts({ total: 0, active: 0, paused: 0 });
	component.setShellCounts({ total: 0, running: 0 });
	return component;
}

test("zero activity appears immediately without keyboard input", () => {
	const component = dock();
	assert.match(plain(component.render(80)), /│ Subagents 0 · Scheduled 0 · Shells 0 +│/);
	assert.equal(component.render(80).length, 3);
	assert.equal(component.isSelectable(), true);
});

test("all three empty groups remain selectable including unavailable subagent details", () => {
	const component = dock();
	component.focused = true;
	const opened: string[] = [];
	component.onOpen = () => opened.push("agents");
	component.onOpenScheduledTasks = () => opened.push("heartbeats");
	component.onOpenShells = () => opened.push("shells");
	component.handleInput("\r");
	component.handleInput("\u001b[B");
	component.handleInput("\r");
	component.handleInput("\t");
	component.handleInput("\r");
	component.handleInput("\u001b[C");
	component.handleInput("\r");
	assert.deepEqual(opened, ["agents", "heartbeats", "shells", "agents"]);
});

test("focus expands details, up and escape return to editor, ordinary text is forwarded", () => {
	const component = dock();
	component.setOpenable(true);
	component.setSubagentCounts({ total: 6, running: 1, idle: 2, inactive: 3 });
	component.setScheduledCounts({ total: 3, active: 1, paused: 2 });
	component.setShellCounts({ total: 5, running: 2, persistent: 1 });
	component.focused = true;
	const output = plain(component.render(120));
	assert.match(output, /1 running · 2 idle · 3 inactive/);
	assert.match(output, /1 active · 2 paused/);
	assert.match(output, /2 running · 5 total · 1 persistent/);
	const actions: string[] = [];
	component.onCancel = () => actions.push("back");
	component.onChatAction = (data) => actions.push(data);
	component.handleInput("\u001b[B");
	component.handleInput("\u001b[A");
	assert.deepEqual(actions, []);
	component.handleInput("\u001b[A");
	component.handleInput("\u001b");
	component.handleInput("hello");
	assert.deepEqual(actions, ["back", "back", "hello"]);
});

test("unknown or failed sources are not presented as zero or stale successful counts", () => {
	const component = dock();
	component.setShellCounts({ total: 5, running: 2 });
	component.setShellState("error");
	component.setScheduledState("unavailable");
	assert.match(plain(component.render(100)), /Subagents 0 · Scheduled unavailable · Shells error/);
	component.focused = true;
	assert.doesNotMatch(plain(component.render(100)), /persistent|2 running|0 active/);
	component.setShellState("loading");
	assert.match(plain(component.render(100)), /Shells · loading/);
	component.setShellCounts({ total: 0, running: 0 });
	assert.match(plain(component.render(100)), /Shells · 0 running · 0 total/);
});

test("narrow widths retain each group and never exceed terminal width", () => {
	const component = dock();
	assert.equal(component.render(20).length, 5);
	for (const focused of [false, true]) {
		component.focused = focused;
		for (const width of [0, 1, 2, 8, 20, 35, 80]) {
			assert.ok(component.render(width).every((line) => visibleWidth(line) <= width));
		}
	}
});

test("picker hides dock and blocks actions without clearing session data", () => {
	initTheme();
	let pickerOpen = true;
	const component = new SubagentSummaryLine(() => "project", () => "model", () => undefined, () => pickerOpen);
	let opened = false;
	component.onOpen = () => { opened = true; };
	assert.deepEqual(component.render(80), []);
	assert.equal(component.isSelectable(), false);
	component.handleInput("\r");
	assert.equal(opened, false);
	pickerOpen = false;
	assert.match(plain(component.render(80)), /project.*model/);
	assert.match(plain(component.render(80)), /Scheduled loading/);
});

test("override label replaces location while preserving context", () => {
	initTheme();
	const component = new SubagentSummaryLine(() => "project", () => "model", () => "override");
	const output = plain(component.render(80));
	assert.match(output, /override.*model/);
	assert.doesNotMatch(output, /project/);
});

 test("Goal is conditional and does not change activity keyboard routing", () => {
 initTheme();
 let goal: string | undefined;
 const component = new SubagentSummaryLine(undefined, undefined, undefined, undefined, () => goal);
 assert.doesNotMatch(plain(component.render(100)), /Goal/);
 goal = "Goal · active (2m)";
 assert.match(plain(component.render(100)), /Goal · active/);
 component.focused = true;
 assert.match(plain(component.render(100)), /Goal · active/);
 const opened: string[] = [];
 component.onOpenShells = () => opened.push("shells");
 component.handleInput("\t"); component.handleInput("\t"); component.handleInput("\r");
 assert.deepEqual(opened, ["shells"]);
 for (const width of [1, 4, 5, 20, 40, 80]) assert.ok(component.render(width).every(line => visibleWidth(line) <= width));
 goal = undefined;
 assert.doesNotMatch(plain(component.render(100)), /Goal/);
 });
