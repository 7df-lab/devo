import assert from "node:assert/strict";
import { test } from "node:test";
import type { TUI } from "@earendil-works/pi-tui";
import { ToolExecutionComponent } from "../../lib/coding-agent/src/modes/interactive/components/tool-execution.js";
import { initTheme } from "../../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { createStreamState, nativeNotificationToPiEvents } from "./events.js";
import { nativeItemsToAgentMessages } from "./messages.js";

const code = 'print("source excerpt")';
const stdout = ["src/example.rs", "35: pub fn example() {", ...Array(216).fill("    // Body"), "}"].join("\n");
const call = {
  id: "call-item",
  item: { type: "toolCall", callId: "call", toolName: "ipython", input: {} },
};
const result = {
  id: "result-item",
  item: {
    type: "toolResult",
    callId: "call",
    output: {
      content: [{ type: "text", text: stdout }],
      details: { stdout, stderr: "", result: null, status: "ok", durationMs: 42 },
    },
    isError: false,
  },
};

function assertPythonRow(pending: Record<string, unknown>, end: Record<string, unknown>) {
  initTheme();
  const ui = { requestRender() {} } as unknown as TUI;
  const cell = new ToolExecutionComponent(String(pending.toolName), "call", pending.args, {}, undefined, ui, process.cwd());
  cell.updateResult({ ...end.result as typeof result.item.output, isError: Boolean(end.isError) });
  const render = () => cell.render(100).join("\n").replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");
  const collapsed = render();
  assert.match(collapsed, /^ ✓ python\b/);
  assert.doesNotMatch(collapsed, /src\/example\.rs|more lines/);
  cell.setExpanded(true);
  assert.match(render(), /35: pub fn example\(\)/);
}

test("early status and input wait for the named call instead of creating a generic row", () => {
  const state = createStreamState();
  const status = { toolCallId: "call", status: "in_progress" };
  const delta = { itemId: "call-item", delta: JSON.stringify({ tool_use_id: "call", partial_json: JSON.stringify({ code }) }) };
  assert.deepEqual(nativeNotificationToPiEvents("tool_call/status_updated", status, state), []);
  assert.deepEqual(nativeNotificationToPiEvents("item/toolCall/inputDelta", delta, state), []);

  const started = nativeNotificationToPiEvents("item/started", call, state);
  assert.deepEqual(started, [
    { type: "tool_pending", toolCallId: "call", toolName: "ipython", args: { code } },
    { type: "tool_args_complete", toolCallId: "call" },
    { type: "tool_execution_start", toolCallId: "call", toolName: "ipython", args: {} },
  ]);
  const ended = nativeNotificationToPiEvents("item/completed", result, state);
  assert.equal(ended.length, 1);
  assert.equal(ended[0]?.toolName, "ipython");
  assertPythonRow(started[0]!, ended[0]!);
  assert.deepEqual(nativeNotificationToPiEvents("tool_call/status_updated", status, state), []);
  assert.deepEqual(nativeNotificationToPiEvents("item/toolCall/inputDelta", delta, state), []);
});

test("result-only Python cells use the Python renderer live and on restore", () => {
  const events = nativeNotificationToPiEvents("item/completed", result, createStreamState());
  assert.equal(events[0]?.toolName, "ipython");
  assertPythonRow(events[0]!, events.at(-1)!);

  const [callMessage, resultMessage] = nativeItemsToAgentMessages([result]);
  const part = (callMessage?.content as Array<{ name: string; arguments: unknown }>)[0]!;
  assert.equal(part.name, "ipython");
  assert.equal(resultMessage?.toolName, "ipython");
  assertPythonRow({ toolName: part.name, args: part.arguments }, { result: resultMessage });
});

test("a known tool name wins over Python-like output; status alone doesn't identify Python", () => {
  const state = createStreamState();
  const custom = { ...call, item: { ...call.item, toolName: "custom" } };
  nativeNotificationToPiEvents("item/started", custom, state);
  assert.equal(nativeNotificationToPiEvents("item/completed", result, state).at(-1)?.toolName, "custom");
  assert.equal(nativeItemsToAgentMessages([custom, result]).at(-1)?.toolName, "custom");
  const namedResult = { ...result, item: { ...result.item, toolName: "custom" } };
  assert.equal(nativeNotificationToPiEvents("item/completed", namedResult, createStreamState()).at(-1)?.toolName, "custom");
  const generic = { ...result, item: { ...result.item, output: { status: "ok" } } };
  assert.equal(nativeNotificationToPiEvents("item/completed", generic, createStreamState()).at(-1)?.toolName, "tool");
});
