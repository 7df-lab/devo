import assert from "node:assert/strict";
import test from "node:test";
import { normalizeAgentModel } from "./native-agent-connection.js";

test("normalizeAgentModel maps ModelBinding id from model field", () => {
  const model = normalizeAgentModel({
    provider: "deepseek",
    model: "deepseek-v4-flash",
  });
  assert.deepEqual(model, {
    id: "deepseek-v4-flash",
    provider: "deepseek",
    name: "deepseek-v4-flash",
    api: "openai-completions",
    baseUrl: "",
    reasoning: false,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 128000,
    maxTokens: 8192,
  });
});

test("normalizeAgentModel splits slug-shaped session ModelBinding provider", () => {
  const model = normalizeAgentModel({
    provider: "deepseek/deepseek-v4-flash",
    model: "deepseek/deepseek-v4-flash",
  });
  assert.equal(model?.id, "deepseek-v4-flash");
  assert.equal(model?.provider, "deepseek");
});

test("normalizeAgentModel keeps capability level names (no max→xhigh remap)", () => {
  const model = normalizeAgentModel({
    provider: "deepseek",
    id: "deepseek-v4-flash",
    reasoningCapability: { levels: ["off", "high", "max"] },
  });
  assert.equal(model?.reasoning, true);
  assert.deepEqual((model as { availableThinkingLevels?: string[] })?.availableThinkingLevels, [
    "off",
    "high",
    "max",
  ]);
});

test("normalizeAgentModel uses thinkingLevelMap like pi getSupportedThinkingLevels", () => {
  // DeepSeek catalog: UI chips off/high/max; xhigh is excluded (null).
  const model = normalizeAgentModel({
    provider: "deepseek",
    id: "deepseek-v4-flash",
    reasoning: true,
    thinkingLevelMap: {
      minimal: null,
      low: null,
      medium: null,
      high: "high",
      xhigh: null,
      max: "max",
    },
  });
  assert.equal(model?.reasoning, true);
  assert.deepEqual((model as { availableThinkingLevels?: string[] })?.availableThinkingLevels, [
    "off",
    "high",
    "max",
  ]);
});

test("normalizeAgentModel prefers availableThinkingLevels from model/list", () => {
  const model = normalizeAgentModel({
    slug: "deepseek/deepseek-v4-flash",
    displayName: "DeepSeek V4 Flash",
    providerId: "deepseek",
    modelId: "deepseek-v4-flash",
    provider: "anthropic_messages",
    reasoning: true,
    contextWindow: 1_000_000,
    maxTokens: 8192,
    availableThinkingLevels: ["off", "high", "max"],
    thinkingLevelMap: {
      high: "high",
      xhigh: null,
      max: "max",
    },
  });
  assert.equal(model?.id, "deepseek-v4-flash");
  assert.equal(model?.provider, "deepseek");
  assert.equal(model?.reasoning, true);
  assert.equal(model?.contextWindow, 1_000_000);
  assert.deepEqual((model as { availableThinkingLevels?: string[] })?.availableThinkingLevels, [
    "off",
    "high",
    "max",
  ]);
});

test("normalizeAgentModel infers reasoning from reasoningCapability.levels", () => {
  const model = normalizeAgentModel({
    provider: "deepseek",
    id: "deepseek-v4-flash",
    reasoningCapability: { levels: ["off", "high", "max"] },
  });
  assert.equal(model?.reasoning, true);
});
