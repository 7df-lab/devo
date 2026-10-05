import assert from "node:assert/strict";
import { test } from "node:test";
import { IPythonCellComponent } from "./ipython-cell.js";
import { initTheme } from "../theme/theme.js";

initTheme("dark");

test("IPythonCellComponent collapsed line shows websearch query, not dump", () => {
	const cell = new IPythonCellComponent({
		code: "print(await websearch.run('Rust async docs', num_results=3))",
		content: [{ type: "text", text: `${"search hit dump\n".repeat(80)}huge tail` }],
		details: {
			stdout: `${"result line\n".repeat(100)}END`,
			status: "ok",
			durationMs: 1200,
		},
		isPartial: false,
		isError: false,
		expanded: false,
		executionStarted: true,
		argsComplete: true,
		showExpandHint: false,
		cwd: process.cwd(),
	});
	const lines = cell.render(100);
	assert.equal(lines.length, 1, `collapsed must be one line, got ${lines.length}: ${JSON.stringify(lines)}`);
	assert.match(lines[0] ?? "", /websearch 'Rust async docs'/);
	assert.ok(!(lines[0] ?? "").includes("huge tail"));
	assert.ok(!(lines[0] ?? "").includes("result line"));
});

test("IPythonCellComponent expanded can show output below the header", () => {
	const cell = new IPythonCellComponent({
		code: "await websearch.run('q')",
		details: { stdout: "one\ntwo\nthree", status: "ok", durationMs: 10 },
		isPartial: false,
		isError: false,
		expanded: true,
		executionStarted: true,
		argsComplete: true,
		showExpandHint: false,
		cwd: process.cwd(),
	});
	const lines = cell.render(100);
	assert.ok(lines.length > 1);
	assert.match(lines[0] ?? "", /websearch 'q'/);
	assert.ok(lines.some((line) => line.includes("one")));
});
