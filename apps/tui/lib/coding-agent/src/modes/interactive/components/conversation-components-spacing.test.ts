import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type { Component, TUI } from "@earendil-works/pi-tui";

import { initTheme } from "../theme/theme.js";
import { AssistantMessageComponent } from "./assistant-message.js";
import { buildConversationComponents, createConversationSpacing } from "./conversation-components.js";
import { ToolExecutionComponent } from "./tool-execution.js";

function assistantMessageWithSpacing(content: "visible" | "tool-only"): AssistantMessageComponent {
	const component = Object.create(AssistantMessageComponent.prototype) as AssistantMessageComponent;
	component.getSpacingContent = () => content;
	component.hasTrailingSpace = () => true;
	return component;
}

test("tool gets its own leading spacer after visible assistant text", () => {
	assert.equal(createConversationSpacing([assistantMessageWithSpacing("visible")]).shouldAddToolLeadingSpace(), true);
});

test("tool-only assistant message does not get a duplicate leading spacer", () => {
	assert.equal(createConversationSpacing([assistantMessageWithSpacing("tool-only")]).shouldAddToolLeadingSpace(), false);
});

test("consecutive tool execution cells have no spacer between them", () => {
	const previousTool = Object.create(ToolExecutionComponent.prototype) as ToolExecutionComponent;
	assert.equal(createConversationSpacing([previousTool]).shouldAddToolLeadingSpace(), false);
});

test("tool-only assistant messages do not split consecutive tool cells", () => {
	const previousTool = Object.create(ToolExecutionComponent.prototype) as ToolExecutionComponent;
	const interstitialAssistant = assistantMessageWithSpacing("tool-only");
	assert.equal(createConversationSpacing([previousTool, interstitialAssistant]).shouldAddToolLeadingSpace(), false);
});

test("hiding streaming thinking removes the blank row between existing tool cells", () => {
	initTheme();
	const first: AssistantMessage = {
		role: "assistant",
		content: [{ type: "toolCall", id: "first", name: "ipython", arguments: { code: "print(1)" } }],
		api: "openai-completions",
		provider: "mock",
		model: "test",
		stopReason: "toolUse",
		timestamp: 1,
		usage: {
			input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
	};
	const second: AssistantMessage = {
		...first,
		content: [
			{ type: "thinking", thinking: "Thinking between tool calls" },
			{ type: "toolCall", id: "second", name: "ipython", arguments: { code: "print(2)" } },
		],
	};
	const messages: AgentMessage[] = [];
	for (const [message, toolCallId] of [[first, "first"], [second, "second"]] as const) {
		messages.push(message, {
			role: "toolResult",
			toolCallId,
			toolName: "ipython",
			content: [{ type: "text", text: "done" }],
			isError: false,
			timestamp: 1,
		});
	}
	const build = (hideThinkingBlock: boolean) => buildConversationComponents(messages, {
		ui: { requestRender() {} } as unknown as TUI,
		cwd: process.cwd(),
		toolOptions: {},
		getToolDefinition: () => undefined,
		hideThinkingBlock,
	});
	const render = (components: Component[]) => components.flatMap((component) => component.render(100))
		.map((line) => stripVTControlCharacters(line).trim());
	const components = build(false);
	const thinking = components.filter((component) => component instanceof AssistantMessageComponent)[1]!;
	thinking.updateContent(second, true);
	const detailsRows = render(components);
	assert.ok(detailsRows.includes("Thinking between tool calls"));

	thinking.setHideThinkingBlock(true);
	const collapsedRows = render(components);
	assert.deepEqual(collapsedRows, render(build(true)));
	assert.match(collapsedRows.at(-2)!, /print\(1\)/);
	assert.match(collapsedRows.at(-1)!, /print\(2\)/);

	thinking.setHideThinkingBlock(false);
	assert.deepEqual(render(components), detailsRows);
});
