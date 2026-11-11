import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "./interactive-mode.js";

test("tree navigation replaces the transcript before forcing a full viewport repaint", async () => {
	for (const draft of ["", "unfinished draft"]) {
		const operations: string[] = [];
		let text = draft;
		const rows = ["abandoned user message", "abandoned response"];
		const mode = Object.create(InteractiveMode.prototype) as {
			renderTreeNavigation(result: { editorText?: string }): Promise<void>;
		};
		Object.assign(mode, {
			sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0,
			treeNavigationRenderGeneration: 0,
			clearSideQuestion: ({ abort }: { abort: boolean }) => { assert.equal(abort, true); operations.push("abort side question"); },
			chatContainer: { clear: () => { rows.length = 0; operations.push("clear"); } },
			renderInitialMessages: async () => { assert.deepEqual(rows, []); rows.push("selected path"); operations.push("replay"); },
			editor: { getText: () => text, setText: (value: string) => { text = value; } },
			showStatus: () => { operations.push("status"); },
			ui: { requestRender: (force: boolean) => { assert.equal(force, true); operations.push("repaint"); } },
		});
		await mode.renderTreeNavigation({ editorText: "selected user message" });
		assert.deepEqual(rows, ["selected path"]);
		assert.equal(text, draft || "selected user message");
		assert.deepEqual(operations, ["abort side question", "clear", "replay", "status", "repaint"]);
	}
});

test("overlapping tree navigation replaces rather than appends and ignores stale editor text", async () => {
	let finishFirst: (() => void) | undefined;
	let calls = 0;
	let text = "";
	const rows: string[] = [];
	const mode = Object.create(InteractiveMode.prototype) as { renderTreeNavigation(result: { editorText?: string }): Promise<void> };
	Object.assign(mode, {
		sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0, treeNavigationRenderGeneration: 0,
		clearSideQuestion: () => {},
		chatContainer: { clear: () => { rows.length = 0; } },
		renderInitialMessages: async () => {
			if (++calls === 1) await new Promise<void>((resolve) => { finishFirst = resolve; });
			rows.push("current path");
		},
		editor: { getText: () => text, setText: (value: string) => { text = value; } },
		showStatus: () => {}, ui: { requestRender: () => {} },
	});
	const first = mode.renderTreeNavigation({ editorText: "old draft" });
	while (!finishFirst) await new Promise((resolve) => setImmediate(resolve));
	const second = mode.renderTreeNavigation({ editorText: "new draft" });
	finishFirst();
	await Promise.all([first, second]);
	assert.deepEqual(rows, ["current path"]);
	assert.equal(text, "new draft");
});

test("session replacement serializes behind navigation and suppresses stale draft restoration", async () => {
	let finish: (() => void) | undefined;
	let text = "";
	const rows: string[] = [];
	const mode = Object.create(InteractiveMode.prototype) as any;
	Object.assign(mode, {
		sessionEventQueue: Promise.resolve(), sessionEventGeneration: 0, treeNavigationRenderGeneration: 0,
		clearSideQuestion: () => {}, chatContainer: { clear: () => { rows.length = 0; } },
		renderInitialMessages: async () => {
			await new Promise<void>((resolve) => { finish = resolve; });
			rows.push("old path");
		},
		editor: { getText: () => text, setText: (value: string) => { text = value; } },
		showStatus: () => { throw new Error("stale navigation status"); }, ui: { requestRender: () => {} },
	});
	const navigation = mode.renderTreeNavigation({ editorText: "old draft" });
	while (!finish) await new Promise((resolve) => setImmediate(resolve));
	++mode.sessionEventGeneration;
	const replacement = mode.sessionEventQueue.then(() => { rows.length = 0; rows.push("new session"); });
	finish();
	await Promise.all([navigation, replacement]);
	assert.deepEqual(rows, ["new session"]);
	assert.equal(text, "");
});
