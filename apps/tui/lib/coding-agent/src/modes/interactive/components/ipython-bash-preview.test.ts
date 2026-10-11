import assert from "node:assert/strict";
import { test } from "node:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import { initTheme } from "../theme/theme.js";
import { IPythonCellComponent } from "./ipython-cell.js";

initTheme("dark");

test("qualified and bare bash calls use the same aligned summary at each pane width", () => {
	for (const width of [20, 40, 80, 110]) {
		for (const expanded of [false, true]) {
			const rows = ['await bash("printf QA_BASH_1012")', 'await rlm.bash("printf QA_BASH_1012")'].map(
				(code) =>
					new IPythonCellComponent({
						code,
						details: { stdout: "QA_BASH_1012", status: "ok", durationMs: 47 },
						executionStarted: true,
						argsComplete: true,
						expanded,
					}).render(width),
			);
			assert.equal(rows[0][0], rows[1][0]);
			assert.match(rows[1][0], /bash/);
			for (const row of rows.flat()) assert.ok(visibleWidth(row) <= width);
		}
	}
});
