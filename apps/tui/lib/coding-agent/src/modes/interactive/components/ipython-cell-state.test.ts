import assert from "node:assert/strict";
import { test } from "node:test";
import { initTheme } from "../theme/theme.js";
import { IPythonCellComponent, type IPythonCellState } from "./ipython-cell.js";

const plain = (lines: string[]): string => lines.join("\n").replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");

for (const [name, state] of Object.entries<IPythonCellState>({
	completed: { code: "", executionStarted: true, isPartial: false, content: [{ type: "text", text: "child result" }] },
	restored: { code: "", content: [{ type: "text", text: "child result" }] },
	failed: { code: "", isError: true, content: [{ type: "text", text: "child error" }] },
	interrupted: { code: "", details: { status: "aborted" } },
})) {
	test(`${name} Python cells without source do not claim code is still pending`, () => {
		initTheme();
		for (const expanded of [false, true]) {
			const cell = new IPythonCellComponent({ ...state, expanded });
			for (const width of [24, 80]) {
				const rendered = plain(cell.render(width));
				assert.doesNotMatch(rendered, /waiting for code/);
				if (expanded && state.content) assert.match(rendered, /child (result|error)/);
			}
		}
	});
}

for (const [name, state] of Object.entries<IPythonCellState>({
	queued: { code: "" },
	running: { code: "", executionStarted: true, isPartial: true },
})) {
	test(`${name} Python cells may still wait for source arguments`, () => {
		initTheme();
		const cell = new IPythonCellComponent({ ...state, expanded: true });
		assert.match(plain(cell.render(80)), /waiting for code/);
	});
}

test("Python source streams before arguments or output complete in compact view", () => {
	initTheme();
	const state: IPythonCellState = { code: "first = 1", expanded: false, isPartial: true, argsComplete: false };
	const cell = new IPythonCellComponent(state);
	for (const width of [40, 100]) assert.match(plain(cell.render(width)), /╰─ first = 1/);
	state.code += "\nsecond = 2\nprint(first + second)";
	cell.update(state);
	for (const width of [40, 100]) {
		const rendered = plain(cell.render(width));
		assert.match(rendered, /second = 2/);
		assert.match(rendered, /print\(first \+ second\)/);
		assert.doesNotMatch(rendered, /waiting for output/);
	}
	cell.update({ ...state, argsComplete: true });
	assert.doesNotMatch(plain(cell.render(100)), /second = 2/);
	cell.update({ ...state, argsComplete: true, expanded: true });
	assert.match(plain(cell.render(100)), /second = 2/);
});

for (const [name, overrides] of Object.entries<Partial<IPythonCellState>>({
	restored: { isPartial: undefined, argsComplete: undefined },
	completed: { isPartial: false },
	running: { executionStarted: true },
	aborted: { details: { status: "aborted" } },
	failed: { isError: true },
})) {
	test(`${name} Python source keeps the compact preference`, () => {
		initTheme();
		const cell = new IPythonCellComponent({ code: "first = 1\nsecond = 2", expanded: false,
			isPartial: true, argsComplete: false, ...overrides });
		assert.doesNotMatch(plain(cell.render(100)), /second = 2/);
	});
}
