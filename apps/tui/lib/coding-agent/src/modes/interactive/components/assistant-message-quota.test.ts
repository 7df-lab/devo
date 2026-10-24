import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import { initTheme } from "../theme/theme.js";
import { AssistantMessageComponent } from "./assistant-message.js";

initTheme("dark");

const quotaMessage = "You have no credits remaining. Please add credits to continue.";

for (const text of ["", "The answer so far."]) {
  test(`quota Error stays singular through streaming, redraw, and restore (${text ? "text" : "empty"})`, () => {
    const streaming: AssistantMessage = {
      role: "assistant",
      content: [{ type: "text", text }],
      api: "openai-responses",
      provider: "mock",
      model: "quota-test",
      timestamp: 1,
      stopReason: "stop",
      usage: {
        input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
    };
    const failed: AssistantMessage = { ...streaming, stopReason: "error", errorMessage: quotaMessage };
    const component = new AssistantMessageComponent();
    const visibleLines = () => component.render(160)
      .map((line) => stripVTControlCharacters(line).trim()).filter(Boolean);
    const body = text ? [text] : [];
    const finalLines = [...body, `Error: ${quotaMessage}`];

    component.updateContent(streaming, true);
    assert.deepEqual(visibleLines(), body);
    component.updateContent(failed, false);
    assert.deepEqual(visibleLines(), finalLines);
    assert.equal(component.getSpacingContent(), "visible");
    // Stable signatures reuse child components; invalidation rebuilds them.
    component.updateContent(failed, false);
    assert.deepEqual(visibleLines(), finalLines);
    component.invalidate();
    assert.deepEqual(visibleLines(), finalLines);
    component.setExpanded(true);
    assert.deepEqual(visibleLines(), finalLines);
    component.setExpanded(false);
    assert.deepEqual(visibleLines(), finalLines);

    const restored = new AssistantMessageComponent(failed);
    assert.deepEqual(restored.render(160).map((line) => stripVTControlCharacters(line).trim()).filter(Boolean), finalLines);
    // Updating the same row to a later successful message must clear stale Error chrome.
    component.updateContent(streaming, false);
    assert.deepEqual(visibleLines(), body);
  });
}
