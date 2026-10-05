import assert from "node:assert/strict";
import { test } from "node:test";
import type { TUI } from "@earendil-works/pi-tui";
import { initTheme } from "../theme/theme.js";
import { formatCollapsedToolPreview, summarizeToolArgs, ToolExecutionComponent } from "./tool-execution.js";

test("summarizeToolArgs shows web_search query", () => {
	assert.equal(summarizeToolArgs("web_search", { query: "Rust async docs" }), "Rust async docs");
	assert.equal(summarizeToolArgs("websearch", { q: "short" }), "short");
});

test("formatCollapsedToolPreview never keeps huge dumps", () => {
	const huge = Array.from({ length: 200 }, (_, i) => `hit ${i}: ${"x".repeat(80)}`).join("\n");
	const preview = formatCollapsedToolPreview(huge);
	assert.ok(preview.length < 320);
	assert.match(preview, /more lines/);
});

const stripAnsi = (text: string): string => text.replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");

test("update_plan renders a dedicated checklist cell", () => {
	initTheme();
	const ui = { requestRender() {} } as unknown as TUI;
	const component = new ToolExecutionComponent(
		"update_plan",
		"plan-1",
		{ plan: [{ step: "Inspect the lifecycle", status: "completed" }] },
		{},
		undefined,
		ui,
		"/tmp",
	);
	component.updateResult({
		content: [{ type: "text", text: "Plan (2 steps):\n[x] Inspect the lifecycle\n[~] Add a regression test" }],
		details: {
			plan: [
				{ step: "Inspect the lifecycle", status: "completed" },
				{ step: "Add a regression test", status: "inProgress" },
			],
		},
		isError: false,
	});

	const renderedLines = component.render(80);
	const rendered = renderedLines.join("\n");
	const plainLines = renderedLines.map(stripAnsi);
	assert.match(plainLines[0] ?? "", /^\s✓ Plan · 2 steps · done$/);
	assert.match(plainLines[1] ?? "", /^\s{2}✓ Inspect the lifecycle$/);
	assert.match(plainLines[2] ?? "", /^\s{2}▶ Add a regression test$/);
	const wrappedLines = component.render(20).map(stripAnsi);
	assert.ok(wrappedLines.some((line) => /^ {4}\S/.test(line)), "wrapped checklist text should align under its task");
	assert.doesNotMatch(rendered, /\u001b\[48;(?:2|5);/);
	assert.doesNotMatch(rendered, /"plan"|update_plan/);
});

test("consecutive Python calls render as compact adjacent rows", () => {
	initTheme();
	const ui = { requestRender() {} } as unknown as TUI;
	const cells = [1, 2, 3].map((index) => {
		const cell = new ToolExecutionComponent(
			"ipython",
			`python-${index}`,
			{ code: `print(${index})` },
			{},
			undefined,
			ui,
			"/tmp",
		);
		cell.markExecutionStarted();
		cell.updateResult({
			content: [{ type: "text", text: String(index) }],
			details: { durationMs: 12 },
			isError: false,
		});
		return cell;
	});
	const renderedRows = cells.flatMap((cell) => cell.render(80));
	assert.equal(renderedRows.length, 3);
	assert.equal(renderedRows.some((line) => line.trim() === ""), false);
	assert.ok(renderedRows.every((line) => line.includes("✓")));
});
