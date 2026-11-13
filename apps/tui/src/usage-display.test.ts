import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { formatContextTree } from "../lib/coding-agent/src/modes/interactive/components/context-tree-format.js";
import { nativeCumulativeUsage, unknownCumulativeUsage } from "./native-to-pi/usage.js";
import { resolveBuiltinSlashCommandName } from "../lib/coding-agent/src/core/slash-commands.js";

test("/context and /usage render provider totals, cache subsets and unknown cost separately from context", () => {
  assert.equal(resolveBuiltinSlashCommandName("usage"), "context");
  const text = formatContextTree({
    id: "root", label: "session", status: "active", ownUsage: null, totalUsage: null, children: [],
    contextUsage: { tokens: 230, contextWindow: 1000, percent: 23 },
    cumulativeUsage: nativeCumulativeUsage({ totalTokens: 350, inputTokens: 300, outputTokens: 50,
      cacheReadInputTokens: 70, cacheCreationInputTokens: 10, callCount: 2, meteredCallCount: 2, unreportedFields: [] }),
  }, 100);
  assert.equal(text, [
    "Context", "", "Current: 230 / 1,000 (23%)", "",
    "Session usage (including child agents and overhead)", "Input: 300", "Output: 50",
    "Cache Read: 70", "Cache Write: 10", "Total: 350",
    "Cache tokens are included in input, not added to the total.",
    "Known estimated cost: unknown", "Metered calls: 2 / 2",
  ].join("\n"));
});

test("unreported metrics render unknown rather than zero", () => {
  const text = formatContextTree({ id: "root", label: "session", status: "active",
    ownUsage: null, totalUsage: null, children: [], cumulativeUsage: unknownCumulativeUsage() }, 80);
  assert.match(text, /Input: unknown\nOutput: unknown\nCache Read: unknown\nCache Write: unknown\nTotal: unknown/);
  assert.match(text, /Known estimated cost: unknown/);
  assert.doesNotMatch(text, /\$0/);
});

test("InteractiveMode applies live context and cumulative events immediately without adding character estimates", () => {
  // Exercise the actual renderer reducer and getters without starting a TTY.
  const mode = Object.create(InteractiveMode.prototype) as any;
  let renders = 0;
  mode.connectionState = { sessionId: "s1", isStreaming: true };
  mode.contextUsageRefresh = { generation: 0, lastSuccessGeneration: 0 };
  mode.ui = { requestRender: () => { renders++; } };
  mode.updateWorkingPulse = () => {};
  mode.activityTracker = { getStatus: () => ({ activity: "writing", direction: "down", tokens: 99 }) };
  const context = { tokens: 230, contextWindow: 1000, percent: 23 };
  mode.updateConnectionStateFromEvent({ type: "context_usage_update", contextUsage: context });
  const cumulativeUsage = nativeCumulativeUsage({ totalTokens: 350, inputTokens: 300, outputTokens: 50 });
  mode.updateConnectionStateFromEvent({ type: "session_usage_update", cumulativeUsage });
  assert.deepEqual(mode.getConnectionContextUsage(), context);
  assert.deepEqual(mode.connectionState.cumulativeUsage, cumulativeUsage);
  assert.equal(renders, 1);
  assert.equal(mode.getWorkingLoaderMessage(), "Writing · ↓ 99 tokens");
});

test("live renderer updates invalidate a slow end-of-turn context refresh", async () => {
  const mode = Object.create(InteractiveMode.prototype) as any;
  let resolveStats!: (value: unknown) => void;
  mode.connectionState = { sessionId: "s1", isStreaming: true };
  mode.contextUsageRefresh = { generation: 0, lastSuccessGeneration: 0 };
  mode.ui = { requestRender: () => {} };
  mode.updateWorkingPulse = () => {};
  mode.agentConnection = { getSessionStats: () => new Promise((resolve) => { resolveStats = resolve; }) };
  const refresh = mode.refreshConnectionContextUsage();
  const latest = { tokens: 230, contextWindow: 1000, percent: 23 };
  mode.updateConnectionStateFromEvent({ type: "context_usage_update", contextUsage: latest });
  resolveStats({ contextUsage: { tokens: 120, contextWindow: 1000, percent: 12 } });
  await refresh;
  assert.deepEqual(mode.getConnectionContextUsage(), latest);
});

test("working token labels preserve counts and directions without an estimate suffix", () => {
  const mode = Object.create(InteractiveMode.prototype) as any;
  mode.connectionState = { isStreaming: true };
  for (const [activity, direction, tokens, expected] of [
    ["writing", "down", 99, "Writing · ↓ 99 tokens"],
    ["executing", "up", 99, "Executing · ↑ 99 tokens"],
    ["waiting", "up", 0, "Waiting"],
  ]) {
    mode.activityTracker = { getStatus: () => ({ activity, direction, tokens }) };
    assert.equal(mode.getWorkingLoaderMessage(), expected);
  }
});
