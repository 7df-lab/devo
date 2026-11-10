import assert from "node:assert/strict";
import { test } from "node:test";
import { createStreamState, nativeNotificationToPiEvents } from "./events.js";
import { nativeCumulativeUsage, newerCumulativeUsage } from "./usage.js";

const totals = {
  totalTokens: 350, inputTokens: 300, outputTokens: 50,
  cacheReadInputTokens: 70, cacheCreationInputTokens: 10,
  callCount: 2, meteredCallCount: 2, unreportedFields: [],
};

test("turn usage projects latest request context separately from cumulative provider tokens", () => {
  assert.deepEqual(nativeNotificationToPiEvents("turn/usage/updated", {
    sessionId: "s1", turnId: "t1", contextWindow: 1000,
    usage: { query: { totalTokens: 230 } }, sessionTotals: totals,
  }, createStreamState()), [
    { type: "context_usage_update", contextUsage: { tokens: 230, contextWindow: 1000, percent: 23 } },
    { type: "session_usage_update", cumulativeUsage: {
      tokens: { input: 300, output: 50, cacheRead: 70, cacheWrite: 10, total: 350 },
      cost: null, currency: null, callCount: 2, meteredCallCount: 2, coverage: "complete",
    } },
  ]);
});

test("session usage reads usage.total and never replaces context with session spend", () => {
  assert.deepEqual(nativeNotificationToPiEvents("session/usage/updated", {
    sessionId: "s1", usage: { total: totals, byPurpose: [] },
  }, createStreamState()), [{ type: "session_usage_update", cumulativeUsage: nativeCumulativeUsage(totals) }]);
});

test("zero/absent query placeholders do not project a context reset", () => {
  for (const query of [undefined, {}, { totalTokens: 0 }]) {
    assert.deepEqual(nativeNotificationToPiEvents("turn/usage/updated", {
      sessionId: "s1", contextWindow: 1000, usage: { query },
    }, createStreamState()), []);
  }
});

test("unknown, partial, historical and known costs remain distinct", () => {
  const missing = nativeCumulativeUsage({ ...totals, totalTokens: 0, meteredCallCount: 0 });
  assert.deepEqual(missing, {
    tokens: { input: null, output: null, cacheRead: null, cacheWrite: null, total: null },
    cost: null, currency: null, callCount: 2, meteredCallCount: 0, coverage: "unreported",
  });
  assert.equal(nativeCumulativeUsage({ ...totals, meteredCallCount: 1 })?.coverage, "partial");
  assert.deepEqual(nativeCumulativeUsage({ totalTokens: 350, callCount: 0, meteredCallCount: 0 })?.tokens, {
    input: null, output: null, cacheRead: null, cacheWrite: null, total: 350,
  });
  assert.deepEqual(nativeCumulativeUsage({ totalTokens: 350 })?.tokens, {
    input: null, output: null, cacheRead: null, cacheWrite: null, total: 350,
  });
  const priced = nativeCumulativeUsage({ ...totals, estimatedCost: { currency: "USD", micros: 1250000 } });
  assert.equal(priced?.cost, 1.25);
  assert.equal(priced?.currency, "USD");
});

test("late cumulative snapshots cannot roll back completed calls", () => {
  const current = nativeCumulativeUsage(totals);
  assert.deepEqual(newerCumulativeUsage(current, nativeCumulativeUsage({ ...totals, totalTokens: 120, callCount: 1 })), current);
  assert.deepEqual(newerCumulativeUsage(current, undefined), current);
});

test("mixed historical spend stays labelled when later live totals omit the legacy detail", () => {
  const historical = nativeCumulativeUsage({ ...totals, totalTokens: 450 }, { totalTokens: 100 });
  assert.equal(historical?.coverage, "legacy");
  assert.deepEqual(historical?.tokens, { input: 300, output: 50, cacheRead: 70, cacheWrite: 10, total: 450 });
  const later = newerCumulativeUsage(historical, nativeCumulativeUsage({ ...totals, totalTokens: 600, callCount: 3, meteredCallCount: 3 }));
  assert.equal(later?.coverage, "legacy");
  assert.equal(later?.legacyTokens, 100);
  assert.equal(later?.tokens.total, 600, "provider total is not reconstructed from the split");
});

test("per-field unknown usage never becomes a fabricated zero while reported zero remains valid", () => {
  const partial = nativeCumulativeUsage({ ...totals, inputTokens: 0, outputTokens: 0,
    unreportedFields: ["inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens"] });
  assert.deepEqual(partial?.tokens, { input: null, output: null, cacheRead: null, cacheWrite: null, total: 350 });
  assert.equal(partial?.coverage, "partial");
  assert.equal(nativeCumulativeUsage({ ...totals, totalTokens: 0, unreportedFields: [] })?.tokens.total, 0);
  assert.equal(nativeCumulativeUsage({ ...totals, unreportedFields: ["totalTokens"] })?.tokens.total, null);
  assert.equal(nativeCumulativeUsage({ ...totals, unreportedFields: undefined })?.coverage, "unknown");
});
