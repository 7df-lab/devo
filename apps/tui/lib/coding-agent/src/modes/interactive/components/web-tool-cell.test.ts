import assert from "node:assert/strict";
import { test } from "node:test";
import { type TUI, visibleWidth } from "@earendil-works/pi-tui";
import { initTheme } from "../theme/theme.js";
import { setWorkingPulseFrame } from "../theme/working-icon.js";
import { ToolExecutionComponent } from "./tool-execution.js";

const ui = { requestRender() {} } as unknown as TUI;
const plain = (lines: string[]) => lines.map(line => line.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, ""));
for (const name of ["web_search", "web_fetch", "web-search", "webfetch", "fetch_url"]) {
	test(`${name} uses a quiet Python-style status row through its lifecycle`, () => {
		initTheme();
		const cell = new ToolExecutionComponent(name, "call", { query: "Rust", url: "https://example.test" }, {}, undefined, ui, process.cwd());
		assert.match(plain(cell.render(100))[0]!, /^ ◇ /);
		assert.equal(cell.render(100).length, 1);
		cell.markExecutionStarted();
		setWorkingPulseFrame(1);
		assert.match(plain(cell.render(100))[0]!, /^ ◈ /);
		setWorkingPulseFrame(2);
		assert.match(plain(cell.render(100))[0]!, /^ ◆ /);
		cell.updateResult({ content: [{ type: "text", text: "Partial page" }], details: {}, isError: false }, true);
		assert.doesNotMatch(plain(cell.render(100))[0]!, /✓/);
		cell.updateResult({ content: [{ type: "text", text: "Page text" }], details: {}, isError: false });
		assert.match(plain(cell.render(100))[0]!, /^ ✓ .*Page text/);
		assert.equal(cell.render(100).length, 1);
		const collapsed = cell.render(100);
		cell.setExpanded(true);
		assert.equal(cell.render(100)[0], collapsed[0]);
		assert.match(plain(cell.render(100)).join("\n"), /╰─ /);
		assert.match(plain(cell.render(100)).join("\n"), / › Page text/);
		cell.setExpanded(false);
		assert.deepEqual(cell.render(100), collapsed);
		cell.updateResult({ content: [{ type: "text", text: "Request failed" }], details: {}, isError: true });
		assert.match(plain(cell.render(100))[0]!, /^ ✗ .*Request failed/);
	});
}

test("web expansion preserves full arguments, bounded details and narrow terminal widths", () => {
	initTheme();
	const query = "中文 documentation ".repeat(20);
	const cell = new ToolExecutionComponent("web_search", "call", { query, limit: 5 }, {}, undefined, ui, process.cwd());
	cell.updateResult({ content: [{ type: "text", text: "1 result" }], details: { resultKind: "web_search", hits: [{ title: "Guide", url: "https://example.test/full", snippet: "Useful excerpt", encrypted_content: "SECRET" }] }, isError: false });
	cell.setExpanded(true);
	const expanded = plain(cell.render(1000)).join("\n");
	assert.ok(expanded.includes(query));
	assert.match(expanded, /limit: 5/);
	assert.match(expanded, /https:\/\/example.test\/full/);
	assert.match(expanded, /Useful excerpt/);
	assert.doesNotMatch(expanded, /SECRET|encrypted_content/);
	for (const width of [1, 2, 3, 10, 40, 80]) {
		assert.ok(cell.render(width).every(line => visibleWidth(line) <= width));
	}
});

test("explicit custom web renderers retain their own shell", () => {
	initTheme();
	const cell = new ToolExecutionComponent("web_search", "call", {}, {}, {
		renderCall: () => ({ render: () => ["CUSTOM WEB CALL"], invalidate() {} }),
	}, ui, process.cwd());
	assert.match(plain(cell.render(100)).join("\n"), /CUSTOM WEB CALL/);
});

for (const [name, args] of [
	["web_fetch", { url: "https://doc.rust-lang.org/book/ch17-02-concurrency-with-async.html" }],
	["web_search", { query: "Rust asynchronous programming documentation and tutorials for beginners" }],
] as const) {
	for (const isError of [false, true]) {
		test(`${name} keeps ${isError ? "failure" : "success"} visible beside long arguments`, () => {
			initTheme();
			const output = isError ? "Request failed: rate limit exceeded" : "FOUND PAGE CONTENT";
			const cell = new ToolExecutionComponent(name, "call", args, {}, undefined, ui, process.cwd());
			cell.updateResult({ content: [{ type: "text", text: output }], isError });
			for (const width of [40, 80]) {
				const collapsed = cell.render(width);
				assert.equal(collapsed.length, 2);
				assert.ok(plain(collapsed)[1]!.includes(output));
				assert.ok(collapsed.every(line => visibleWidth(line) <= width));
				cell.setExpanded(true);
				assert.deepEqual(cell.render(width).slice(0, 2), collapsed);
				cell.setExpanded(false);
				assert.deepEqual(cell.render(width), collapsed);
			}
			for (const width of [1, 2, 3, 10, 20]) {
				assert.ok(cell.render(width).every(line => visibleWidth(line) <= width));
			}
		});
	}
}
