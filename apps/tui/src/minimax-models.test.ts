import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { calculateCost, getModel, getModels, getProviders, getSupportedThinkingLevels } from "../lib/ai/src/models.js";
import { streamSimpleAnthropic } from "../lib/ai/src/providers/anthropic.js";
import type { SimpleStreamOptions, Usage } from "../lib/ai/src/types.js";
import { defaultModelPerProvider } from "../lib/coding-agent/src/core/model-resolver.js";

for (const provider of ["minimax", "minimax-cn"] as const) {
	test(`${provider} offers only M3 and Flash with supported input and thinking modes`, () => {
		assert.deepEqual(getModels(provider).map((model) => model.id).sort(), ["MiniMax-M3", "MiniMax-M3.1-Flash"]);
		assert.equal(defaultModelPerProvider[provider], "MiniMax-M3.1-Flash");
		for (const model of getModels(provider)) {
			assert.equal(model.contextWindow, 1_000_000);
			assert.equal(model.maxTokens, 524_288);
			assert.deepEqual(model.input, ["text", "image"]);
			assert.equal(model.cost.cacheWrite, undefined);
		}
		assert.deepEqual(getSupportedThinkingLevels(getModel(provider, "MiniMax-M3")), ["off", "high"]);
		const flash = getModel(provider, "MiniMax-M3.1-Flash");
		assert.deepEqual(getSupportedThinkingLevels(flash), ["low", "medium", "high", "xhigh", "max"]);
		assert.deepEqual(flash.cost, { input: 0.15, output: 0.6, cacheRead: 0.015 });
	});
}

test("all builtin routes exclude retired MiniMax models", () => {
	for (const provider of getProviders()) {
		for (const model of getModels(provider)) {
			assert.doesNotMatch(model.id, /minimax[./-]m[12](?:\D|$)/i);
		}
	}
	const catalog = JSON.parse(readFileSync(new URL("../../../crates/core/providers.json", import.meta.url), "utf8"));
	for (const [provider, entry] of Object.entries(catalog.provider) as [string, { models: Record<string, any> }][]) {
		for (const id of Object.keys(entry.models)) assert.doesNotMatch(id, /minimax[./-]m[12](?:\D|$)/i);
		if (provider === "minimax" || provider === "minimax-cn") {
			assert.deepEqual(Object.keys(entry.models).sort(), ["MiniMax-M3", "MiniMax-M3.1-Flash"]);
			assert.equal(entry.models["MiniMax-M3.1-Flash"].default_reasoning_selection, "max");
			assert.equal(entry.models["MiniMax-M3.1-Flash"].thinking_level_map.off, null);
			assert.equal(entry.models["MiniMax-M3"].default_reasoning_selection, "off");
		}
	}
});

async function payload(modelId: "MiniMax-M3" | "MiniMax-M3.1-Flash", reasoning?: SimpleStreamOptions["reasoning"]) {
	let captured: any;
	const stream = streamSimpleAnthropic(getModel("minimax", modelId), {
		systemPrompt: "Be concise.",
		messages: [{ role: "user", content: "Hello", timestamp: 0 }],
	}, {
		apiKey: "test-placeholder",
		reasoning,
		serviceTier: "priority",
		onPayload: (body) => {
			captured = body;
			throw new Error("Captured before network request");
		},
	});
	await stream.result();
	assert.ok(captured, "request was captured before network access");
	assert.equal(JSON.stringify(captured).includes("cache_control"), false);
	assert.equal(captured.service_tier, undefined);
	return captured;
}

test("Flash always sends adaptive thinking, defaults to max, and preserves supported efforts", async () => {
	for (const reasoning of [undefined, "off", "low", "medium", "high", "xhigh", "max"] as const) {
		const body = await payload("MiniMax-M3.1-Flash", reasoning);
		assert.deepEqual(body.thinking, { type: "adaptive" });
		assert.deepEqual(body.output_config, { effort: reasoning && reasoning !== "off" ? reasoning : "max" });
	}
});

test("M3 defaults off, enables adaptive thinking, and omits ignored effort controls", async () => {
	assert.deepEqual((await payload("MiniMax-M3")).thinking, { type: "disabled" });
	const body = await payload("MiniMax-M3", "high");
	assert.deepEqual(body.thinking, { type: "adaptive" });
	assert.equal(body.output_config, undefined);
});

test("unpublished cache-write pricing does not produce NaN cost totals", () => {
	const usage: Usage = {
		input: 1_000_000, output: 1_000_000, cacheRead: 1_000_000, cacheWrite: 0, totalTokens: 3_000_000,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	};
	const cost = calculateCost(getModel("minimax", "MiniMax-M3.1-Flash"), usage);
	assert.equal(cost.total, 0.765);
	assert.equal(cost.cacheWrite, 0);
});
