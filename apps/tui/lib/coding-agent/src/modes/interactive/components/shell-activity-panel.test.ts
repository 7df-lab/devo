import assert from "node:assert/strict";
import { test } from "node:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import type { AgentConnectionShellTask } from "../../agent-connection/types.js";
import { initTheme } from "../theme/theme.js";
import { ShellActivityPanel } from "./shell-activity-panel.js";
import type { ActivityLoadState } from "./subagent-summary-line.js";

const plain = (lines: string[]) => lines.join("\n").replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");

function setup() {
	initTheme();
	const state = { tasks: [] as AgentConnectionShellTask[], status: "ready" as ActivityLoadState, error: undefined as string | undefined, rows: 8, renders: 0, closes: 0 };
	const panel = new ShellActivityPanel({ getTasks: () => state.tasks, getRows: () => state.rows,
		getError: () => state.error, getState: () => state.status,
		onClose: () => { state.closes++; }, requestRender: () => { state.renders++; } });
	return { state, panel };
}

test("empty, loading, unsupported and error states are distinguished", () => {
	const { state, panel } = setup();
	assert.match(plain(panel.render(100)), /No background shell tasks/);
	state.status = "loading";
	assert.match(plain(panel.render(100)), /Loading shell activity/);
	assert.doesNotMatch(plain(panel.render(100)), /No background/);
	state.status = "unavailable";
	assert.match(plain(panel.render(100)), /unavailable on this connection/);
	state.status = "error";
	state.error = "connection lost";
	assert.match(plain(panel.render(100)), /Unable to refresh shell activity: connection lost/);
});

test("read-only selection and details reflect fresh task state", () => {
	const { state, panel } = setup();
	state.tasks = [{ id: "one", command: "sleep 30", state: "running" }, { id: "two", command: "build", state: "completed", exitCode: 0, cwd: "/project", output: "done" }];
	panel.render(80);
	panel.handleInput("\u001b[B");
	panel.handleInput("\r");
	assert.match(plain(panel.render(80)), /build\ncompleted · exit 0\n\/project\ndone/);
	state.tasks[1] = { ...state.tasks[1]!, state: "failed", exitCode: 1, output: "failed" };
	assert.match(plain(panel.render(80)), /failed · exit 1/);
	panel.handleInput("\u001b");
	assert.equal(state.closes, 0);
	panel.handleInput("\u001b");
	assert.equal(state.closes, 1);
});

test("selection is stable on reordered updates and removed tasks close detail", () => {
	const { state, panel } = setup();
	state.tasks = [{ id: "one", command: "first", state: "running" }, { id: "two", command: "second", state: "running" }];
	panel.render(80);
	panel.handleInput("\u001b[B");
	state.tasks.reverse();
	panel.handleInput("\r");
	assert.match(plain(panel.render(80)), /second\nrunning/);
	state.tasks = [];
	assert.match(plain(panel.render(80)), /No background shell tasks/);
	panel.handleInput("\u001b");
	assert.equal(state.closes, 1);
});

test("output and list scrolling stay within terminal bounds", () => {
	const { state, panel } = setup();
	state.tasks = Array.from({ length: 20 }, (_, index) => ({ id: String(index), command: `task ${index}`, state: "running" as const, output: "first\nsecond\nthird\nfourth\nfifth" }));
	state.rows = 6;
	panel.render(80);
	for (let i = 0; i < 19; i++) panel.handleInput("\u001b[B");
	assert.match(plain(panel.render(80)), /› running · task 19/);
	panel.handleInput("\r");
	panel.handleInput("\u001b[B");
	assert.match(plain(panel.render(80)), /second/);
	assert.doesNotMatch(plain(panel.render(80)), /first/);
	for (const width of [0, 1, 2, 8, 80]) {
		const lines = panel.render(width);
		assert.ok(lines.length <= state.rows);
		assert.ok(lines.every((line) => visibleWidth(line) <= width));
	}
});

test("command and output cannot inject terminal control sequences", () => {
	const { state, panel } = setup();
	state.tasks = [{ id: "one", command: "echo\u001b[2Jboom\nnext", state: "running", output: "safe\u001b]52;c;abc\u0007\u001b[2Jtext\u0000" }];
	assert.match(plain(panel.render(100)), /echo boom|echoboom/);
	panel.handleInput("\r");
	const output = plain(panel.render(100));
	assert.doesNotMatch(output, /\u0000|\u0007|\u001b|52;c/);
	assert.match(output, /safetext/);
});
