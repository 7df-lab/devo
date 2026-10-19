/**
 * Trace: tool lifecycle projection (pending → args → start → end).
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createNativeToPiToolState,
  displayToolName,
  editResultFromFileChange,
  fileChangeSummariesFromWorkspaceViews,
  normalizeToolArgs,
  planResultFromItem,
  projectToolCallInputDelta,
  projectToolItemStarted,
  projectToolStatusUpdated,
  shapeToolResultOutput,
  summarizeWebSearchOutput,
} from "./tools.js";

test("displayToolName maps exec_command to bash", () => {
  assert.equal(displayToolName("exec_command"), "bash");
  assert.equal(displayToolName("apply_patch"), "edit");
  assert.equal(displayToolName("ipython"), "ipython");
});

test("shapeToolResultOutput summarizes hosted web_search hit arrays", () => {
  const hits = [
    { title: "Devo docs", url: "https://example.com/a" },
    { title: "Coding agent", url: "https://example.com/b" },
    { title: "Extra", url: "https://example.com/c" },
  ];
  const shaped = shapeToolResultOutput(hits, "status: completed");
  assert.equal(shaped.content.length, 1);
  assert.match(shaped.content[0]?.text ?? "", /3 results: Devo docs; Coding agent; Extra/);
  assert.equal(shaped.details?.resultKind, "web_search");
  assert.equal(shaped.details?.hitCount, 3);
  assert.ok(!shaped.content[0]?.text?.includes("https://example.com"));
  assert.equal(summarizeWebSearchOutput(hits).startsWith("3 results:"), true);
});

test("shapeToolResultOutput summarizes JSON-string web_search dumps", () => {
  const hits = [
    { title: "One", url: "https://example.com/1" },
    { title: "Two", url: "https://example.com/2" },
  ];
  const shaped = shapeToolResultOutput(JSON.stringify(hits));
  assert.match(shaped.content[0]?.text ?? "", /2 results: One; Two/);
  assert.equal(shaped.details?.resultKind, "web_search");
});

test("shapeToolResultOutput truncates huge plain strings", () => {
  const shaped = shapeToolResultOutput("z".repeat(5000));
  assert.ok((shaped.content[0]?.text?.length ?? 0) <= 400);
  assert.equal(shaped.details?.resultKind, "truncated_output");
});

test("normalizeToolArgs promotes cmd to command for bash", () => {
  assert.deepEqual(normalizeToolArgs("exec_command", { cmd: "ls" }), {
    command: "ls",
    cmd: "ls",
  });
});

test("shapeToolResultOutput decodes kernel shell result envelopes", () => {
  // Exact envelope shape from a live kernel toolResult (shell_command output).
  const shaped = shapeToolResultOutput({
    command: "seq 1 30; echo EXIT:$?",
    exit: 0,
    description: "Run the exact requested shell command",
    cwd: "/home/wqa/devo",
    yield_time_ms: 1000,
    duration_ms: 3,
    output: "1\n2\n3\nEXIT:0\n",
  });
  assert.deepEqual(shaped, {
    content: [{ type: "text", text: "1\n2\n3\nEXIT:0\n" }],
    details: {
      exitCode: 0,
      durationMs: 3,
      command: "seq 1 30; echo EXIT:$?",
    },
  });
});

test("shapeToolResultOutput decodes JSON-string kernel shell envelopes", () => {
  const shaped = shapeToolResultOutput(
    JSON.stringify({ command: "ls", exit: 2, duration_ms: 12, output: "" }),
  );
  assert.deepEqual(shaped, {
    content: [{ type: "text", text: "(no output)" }],
    details: { exitCode: 2, durationMs: 12, command: "ls" },
  });
});

test("normalizeToolArgs projects kernel shell timeout milliseconds to seconds", () => {
  assert.deepEqual(normalizeToolArgs("shell_command", { command: "ls", timeout: 120000 }), {
    command: "ls",
    timeout: 120,
  });
  assert.deepEqual(normalizeToolArgs("shell_command", { command: "ls", timeout: 1500 }), {
    command: "ls",
    timeout: 1.5,
  });
  assert.deepEqual(normalizeToolArgs("shell_command", { command: "ls" }), { command: "ls" });
});

test("inputDelta accumulates partial_json by itemId", () => {
  const state = createNativeToPiToolState();
  projectToolItemStarted(
    { id: "item_1" },
    { type: "toolCall", callId: "c1", toolName: "ipython", arguments: {} },
    state,
  );
  const first = projectToolCallInputDelta(
    {
      itemId: "item_1",
      delta: JSON.stringify({ tool_use_id: "c1", partial_json: '{"code":"ab' }),
    },
    state,
  );
  assert.equal(first[0]?.type, "tool_args_update");
  assert.equal((first[0]?.args as { code?: string }).code, "ab");

  const second = projectToolCallInputDelta(
    {
      itemId: "item_1",
      delta: JSON.stringify({ tool_use_id: "c1", partial_json: 'c"}' }),
    },
    state,
  );
  assert.equal((second[0]?.args as { code?: string }).code, "abc");
});

test("status in_progress emits args_complete then execution_start", () => {
  const state = createNativeToPiToolState();
  projectToolItemStarted(
    { id: "item_1" },
    { type: "toolCall", callId: "c1", toolName: "ipython", arguments: { code: "1" } },
    state,
  );
  const events = projectToolStatusUpdated({ toolCallId: "c1", status: "in_progress" }, state);
  assert.deepEqual(
    events.map((e) => e.type),
    ["tool_args_complete", "tool_execution_start"],
  );
});

test("editResultFromFileChange maps unifiedDiff for edit renderer", () => {
  const shaped = editResultFromFileChange({
    type: "fileChange",
    callId: "c1",
    changes: [
      {
        path: "src/a.ts",
        change: { type: "update", unifiedDiff: "--- a\n+++ b\n@@\n-old\n+new\n" },
      },
    ],
  });
  assert.equal(shaped.details.path, "src/a.ts");
  assert.match(String(shaped.details.diff), /\+new/);
});

test("fileChangeSummariesFromWorkspaceViews maps additions/deletions", () => {
  const changes = fileChangeSummariesFromWorkspaceViews({
    views: [
      {
        files: [
          { path: "a.ts", additions: 2, deletions: 1, status: "modified" },
          { path: "b.ts", status: "added" },
        ],
      },
    ],
  });
  assert.deepEqual(changes, [
    { path: "a.ts", added: 2, removed: 1 },
    { path: "b.ts", added: 1, removed: 0 },
  ]);
});

test("plan item start emits update_plan tool_pending keyed by callId", () => {
  const state = createNativeToPiToolState();
  const events = projectToolItemStarted(
    { id: "item_plan_1" },
    { type: "plan", callId: "call-plan-1", entries: [] },
    state,
  );
  assert.deepEqual(events, [
    { type: "tool_pending", toolCallId: "call-plan-1", toolName: "update_plan", args: {} },
  ]);
  // status_updated (real call id) must land on the same row, not a second one.
  const statusEvents = projectToolStatusUpdated(
    { toolCallId: "call-plan-1", status: "in_progress" },
    state,
  );
  assert.deepEqual(
    statusEvents.map((e) => e.type),
    ["tool_args_complete", "tool_execution_start"],
  );
  assert.equal(
    statusEvents[statusEvents.length - 1]?.toolName,
    "update_plan",
    "plan row must not degrade to a generic 'tool' name",
  );
});

test("legacy plan item start falls back to itemId; inputDelta remaps to tool_use_id", () => {
  const state = createNativeToPiToolState();
  projectToolItemStarted({ id: "item_plan_2" }, { type: "plan", entries: [] }, state);
  const events = projectToolCallInputDelta(
    {
      itemId: "item_plan_2",
      delta: JSON.stringify({ tool_use_id: "call-plan-2", partial_json: '{"plan": [{' }),
    },
    state,
  );
  assert.ok(
    events.every((e) => e.toolCallId === "call-plan-2"),
    "deltas after the remap must key the real tool call id",
  );
});

test("planResultFromItem renders checklist and preserves entries", () => {
  const shaped = planResultFromItem({
    type: "plan",
    callId: "c1",
    entries: [
      { step: "Verify-R8-PLAN-CREATE", status: "inProgress" },
      { step: "Probe-R8-PLAN-UPDATE", status: "pending" },
      { step: "Confirm-R8-PLAN-PERSIST", status: "completed" },
    ],
  });
  const text = shaped.content[0]?.text as string;
  assert.match(text, /Plan \(3 steps\)/);
  assert.match(text, /\[~\] Verify-R8-PLAN-CREATE/);
  assert.match(text, /\[ \] Probe-R8-PLAN-UPDATE/);
  assert.match(text, /\[x\] Confirm-R8-PLAN-PERSIST/);
  assert.deepEqual(shaped.details.plan, [
    { step: "Verify-R8-PLAN-CREATE", status: "inProgress" },
    { step: "Probe-R8-PLAN-UPDATE", status: "pending" },
    { step: "Confirm-R8-PLAN-PERSIST", status: "completed" },
  ]);
});

test("planResultFromItem renders proposed-plan markdown verbatim", () => {
  const markdown = "## Approach\n\n1. Inspect\n2. Patch";
  const shaped = planResultFromItem({
    type: "plan",
    entries: [{ step: markdown, status: "completed" }],
  });
  assert.equal(shaped.content[0]?.text, markdown);
});
