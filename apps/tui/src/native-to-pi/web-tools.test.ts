import assert from "node:assert/strict";
import { test } from "node:test";
import type { TUI } from "@earendil-works/pi-tui";
import { initTheme } from "../../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { ToolExecutionComponent } from "../../lib/coding-agent/src/modes/interactive/components/tool-execution.js";
import { createStreamState, nativeNotificationToPiEvents } from "./events.js";
import { nativeItemsToAgentMessages } from "./messages.js";
import { normalizeToolArgs, shapeToolResultOutput } from "./tools.js";

const hits = [
  { title: "Rust async guide", url: "https://example.test/rust", snippet: "Async guide excerpt", encrypted_content: "opaque-provider-data" },
  { title: "Rust reference", url: "https://example.test/reference" },
];
const ui = { requestRender() {} } as unknown as TUI;
const plain = (component: ToolExecutionComponent, width = 100) =>
  component.render(width).join("\n").replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, "");

for (const [toolName, input, output] of [
  ["web_search", { query: "Rust async" }, hits],
  ["web_fetch", { url: "https://example.test/rust" }, "Fetched page text"],
  ["image_generation", { prompt: "A landscape" }, "Generated image"],
] as const) {
  for (const type of ["toolCall", "hostedToolCall"]) {
    test(`${toolName} ${type} refreshes final arguments and matches restored rendering`, () => {
      initTheme();
      const state = createStreamState();
      const started = { id: "item-1", item: { type, callId: "call-1", toolName, input: {} } };
      const completed = { ...started, state: "completed", item: { ...started.item, input, ...(type === "hostedToolCall" ? { output } : {}) } };
      const result = { id: "result-1", item: { type: "toolResult", callId: "call-1", output, isError: false } };
      const events = [
        ...nativeNotificationToPiEvents("item/started", started, state),
        ...nativeNotificationToPiEvents("item/completed", completed, state),
        ...(type === "toolCall" ? nativeNotificationToPiEvents("item/completed", result, state) : []),
      ];
      const updates = events.filter((event) => event.type === "tool_args_update");
      assert.deepEqual(updates, [{ type: "tool_args_update", toolCallId: "call-1", toolName, args: input }]);
      const end = events.find((event) => event.type === "tool_execution_end")!;
      assert.equal(events.filter((event) => event.type === "tool_pending").length, 1);
      assert.equal(events.filter((event) => event.type === "tool_execution_end").length, 1);
      const live = new ToolExecutionComponent(toolName, "call-1", updates[0]!.args, {}, undefined, ui, process.cwd());
      live.updateResult({ ...end.result as ReturnType<typeof shapeToolResultOutput>, isError: Boolean(end.isError) });
      const messages = nativeItemsToAgentMessages(type === "toolCall" ? [completed, result] : [completed]);
      const call = (messages[0]!.content as Array<{ arguments: unknown }>)[0]!;
      const restored = new ToolExecutionComponent(toolName, "call-1", call.arguments, {}, undefined, ui, process.cwd());
      restored.updateResult(messages[1] as unknown as Parameters<ToolExecutionComponent["updateResult"]>[0]);
      assert.equal(plain(live), plain(restored));
      assert.ok(plain(live).includes(Object.values(input)[0]!));
      assert.match(plain(live), toolName === "web_search" ? /2 results: Rust async guide; Rust reference/ : new RegExp(String(output)));
      assert.deepEqual(nativeNotificationToPiEvents("item/completed", completed, state), []);
      assert.deepEqual(nativeNotificationToPiEvents("item/updated", completed, state), []);
    });
  }
}

test("tool item updates preserve query and URL before completion", () => {
  for (const type of ["toolCall", "hostedToolCall"]) {
    const state = createStreamState();
    const envelope = { id: "item", item: { type, callId: "call", toolName: "web_fetch", input: {} } };
    nativeNotificationToPiEvents("item/started", envelope, state);
    const events = nativeNotificationToPiEvents("item/updated", {
      ...envelope, item: { ...envelope.item, input: { url: "https://example.test" } },
    }, state);
    assert.deepEqual(events[0], { type: "tool_args_update", toolCallId: "call", toolName: "web_fetch", args: { url: "https://example.test" } });
  }
});

test("hosted completion without a start still carries its arguments", () => {
  const events = nativeNotificationToPiEvents("item/completed", {
    id: "hosted", item: { type: "hostedToolCall", callId: "call", toolName: "web_search", input: { query: "Rust" }, output: hits },
  }, createStreamState());
  assert.deepEqual(events[0], { type: "tool_pending", toolCallId: "call", toolName: "web_search", args: { query: "Rust" } });
  assert.equal(events.at(-1)?.type, "tool_execution_end");
});

test("legacy hosted calls without call IDs use the envelope ID live and on restore", () => {
  for (const callId of [undefined, ""]) {
    const envelope = { id: "legacy-web", state: "completed", item: { type: "hostedToolCall", callId, toolName: "web_search", output: hits } };
    const events = nativeNotificationToPiEvents("item/completed", envelope, createStreamState());
    const messages = nativeItemsToAgentMessages([envelope]);
    assert.ok(events.every((event) => event.toolCallId === "legacy-web"));
    assert.equal(messages.length, 2);
    assert.equal(messages[1]?.toolCallId, "legacy-web");
    assert.equal(messages[1]?.isError, false);
  }
});

for (const state of ["completed", "failed", "interrupted", "lost"]) {
  test(`hosted ${state} with no output has the same terminal result live and on restore`, () => {
    const envelope = { id: "hosted-empty", state, item: { type: "hostedToolCall", callId: "call", toolName: "web_fetch", input: { url: "https://example.test" } } };
    const stream = createStreamState();
    const events = nativeNotificationToPiEvents("item/completed", envelope, stream);
    const end = events.at(-1)!;
    const result = nativeItemsToAgentMessages([envelope])[1]!;
    assert.equal(end.type, "tool_execution_end");
    assert.equal(end.isError, state !== "completed");
    assert.equal(result.isError, end.isError);
    assert.deepEqual(result.content, (end.result as { content: unknown }).content);
    assert.equal(stream.tools.pendingCalls.size, 0);
  });
}

test("search and fetch arguments accept JSON-encoded or plain provider inputs", () => {
  assert.deepEqual(normalizeToolArgs("web_search", '{"query":"Rust docs"}'), { query: "Rust docs" });
  assert.deepEqual(normalizeToolArgs("web-search", "Rust docs"), { query: "Rust docs" });
  assert.deepEqual(normalizeToolArgs("web-search", "2026"), { query: "2026" });
  assert.deepEqual(normalizeToolArgs("web_fetch", "https://example.test"), { url: "https://example.test" });
});

test("JSON search results retain expandable hits even with terse display text", () => {
  const output = JSON.stringify(hits.map((hit) => ({ ...hit, encrypted_content: "x".repeat(900) })));
  const result = shapeToolResultOutput(output, "status: completed");
  initTheme();
  const cell = new ToolExecutionComponent("web_search", "call", { query: "Rust" }, {}, undefined, ui, process.cwd());
  cell.updateResult({ ...result, isError: false });
  const collapsed = plain(cell);
  assert.match(collapsed, /2 results: Rust async guide; Rust reference/);
  assert.doesNotMatch(collapsed, /https:|encrypted_content|status: completed/);
  cell.setExpanded(true);
  assert.match(plain(cell), /https:\/\/example.test\/rust/);
  assert.match(plain(cell), /Async guide excerpt/);
  assert.doesNotMatch(plain(cell), /encrypted_content|opaque-provider-data|x{100}/);
  cell.setExpanded(false);
  assert.equal(plain(cell), collapsed);
});

test("search results with status metadata are not mistaken for Python results", () => {
  const result = shapeToolResultOutput({ status: "completed", results: hits });
  assert.deepEqual(result.content, [{ type: "text", text: "2 results: Rust async guide; Rust reference" }]);
  assert.equal(result.details?.resultKind, "web_search");
});

test("fetch text blocks remain text and are not mislabeled search hits", () => {
  const content = [{ type: "text", text: "Fetched document" }];
  assert.deepEqual(shapeToolResultOutput(content), { content });
  assert.deepEqual(shapeToolResultOutput(JSON.stringify(content)), { content });
  for (const field of ["text", "output"]) {
    const result = shapeToolResultOutput(JSON.stringify({ [field]: "Fetched document" }), "status: completed");
    assert.deepEqual(result.content, content);
  }
});

test("fetch document envelopes render page text instead of transport JSON", () => {
  const output = { type: "web_fetch_result", url: "https://example.test", content: { type: "document", title: "Guide", source: { type: "text", media_type: "text/plain", data: "Readable page text" } } };
  const expected = { content: [{ type: "text", text: "Guide\nReadable page text" }], details: { resultKind: "web_fetch", url: "https://example.test" } };
  assert.deepEqual(shapeToolResultOutput(output), expected);
  assert.deepEqual(shapeToolResultOutput(JSON.stringify(output)), expected);
});
