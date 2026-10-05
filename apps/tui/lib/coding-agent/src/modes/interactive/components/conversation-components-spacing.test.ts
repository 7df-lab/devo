import assert from "node:assert/strict";
import { test } from "node:test";

import { AssistantMessageComponent } from "./assistant-message.js";
import { shouldAddLeadingSpaceBeforeToolExecution } from "./conversation-components.js";
import { ToolExecutionComponent } from "./tool-execution.js";

function assistantMessageWithSpacing(content: "visible" | "tool-only"): AssistantMessageComponent {
	const component = Object.create(AssistantMessageComponent.prototype) as AssistantMessageComponent;
	component.getSpacingContent = () => content;
	component.hasTrailingSpace = () => true;
	return component;
}

test("tool gets its own leading spacer after visible assistant text", () => {
	assert.equal(shouldAddLeadingSpaceBeforeToolExecution([assistantMessageWithSpacing("visible")]), true);
});

test("tool-only assistant message does not get a duplicate leading spacer", () => {
	assert.equal(shouldAddLeadingSpaceBeforeToolExecution([assistantMessageWithSpacing("tool-only")]), false);
});

test("consecutive tool execution cells have no spacer between them", () => {
	const previousTool = Object.create(ToolExecutionComponent.prototype) as ToolExecutionComponent;
	assert.equal(shouldAddLeadingSpaceBeforeToolExecution([previousTool]), false);
});

test("tool-only assistant messages do not split consecutive tool cells", () => {
	const previousTool = Object.create(ToolExecutionComponent.prototype) as ToolExecutionComponent;
	const interstitialAssistant = assistantMessageWithSpacing("tool-only");
	assert.equal(shouldAddLeadingSpaceBeforeToolExecution([previousTool, interstitialAssistant]), false);
});
