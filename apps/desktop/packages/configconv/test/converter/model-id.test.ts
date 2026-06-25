import { describe, expect, test } from "bun:test"
import {
	detectProvider,
	isValidModelId,
	suggestSmallModel,
	translateModelId,
} from "../../src/converter/model-id"

describe("translateModelId", () => {
	test("maps known Anthropic models", () => {
		expect(translateModelId("claude-opus-4-6")).toBe("anthropic/claude-opus-4-6")
		expect(translateModelId("claude-sonnet-4-5")).toBe("anthropic/claude-sonnet-4-5")
		expect(translateModelId("claude-sonnet-4")).toBe("anthropic/claude-sonnet-4")
		expect(translateModelId("claude-3-5-haiku-20241022")).toBe(
			"anthropic/claude-3-5-haiku-20241022",
		)
	})

	test("maps short aliases", () => {
		expect(translateModelId("opus")).toBe("anthropic/claude-opus-4-6")
		expect(translateModelId("sonnet")).toBe("anthropic/claude-sonnet-4-5")
		expect(translateModelId("haiku")).toBe("anthropic/claude-3-5-haiku-20241022")
	})

	test("passes through already-qualified model IDs", () => {
		expect(translateModelId("anthropic/claude-opus-4-6")).toBe("anthropic/claude-opus-4-6")
		expect(translateModelId("amazon-bedrock/some-model")).toBe("amazon-bedrock/some-model")
	})

	test("handles Bedrock ARN-style model IDs", () => {
		expect(translateModelId("anthropic.claude-opus-4-6-v1:0:1m")).toBe(
			"amazon-bedrock/anthropic.claude-opus-4-6-v1:0:1m",
		)
		expect(translateModelId("us.anthropic.claude-sonnet-4-5-v1:0")).toBe(
			"amazon-bedrock/us.anthropic.claude-sonnet-4-5-v1:0",
		)
	})

	test("applies manual overrides", () => {
		const overrides = { "my-custom-model": "custom/my-model" }
		expect(translateModelId("my-custom-model", undefined, overrides)).toBe("custom/my-model")
	})

	test("overrides take precedence over built-in map", () => {
		const overrides = { opus: "custom/opus-override" }
		expect(translateModelId("opus", undefined, overrides)).toBe("custom/opus-override")
	})

	test("unknown models use detected provider as prefix", () => {
		expect(translateModelId("unknown-model")).toBe("anthropic/unknown-model")
		expect(translateModelId("unknown-model", "google-vertex")).toBe("google-vertex/unknown-model")
	})

	test("claude- prefix defaults to anthropic", () => {
		expect(translateModelId("claude-future-model-2027")).toBe("anthropic/claude-future-model-2027")
	})
})

describe("detectProvider", () => {
	test("defaults to anthropic", () => {
		expect(detectProvider()).toBe("anthropic")
		expect(detectProvider({})).toBe("anthropic")
	})

	test("detects Bedrock from env", () => {
		expect(detectProvider({ CLAUDE_CODE_USE_BEDROCK: "1" })).toBe("amazon-bedrock")
	})

	test("detects Vertex from env", () => {
		expect(detectProvider({ CLAUDE_CODE_USE_VERTEX: "1" })).toBe("google-vertex")
	})

	test("detects Bedrock from model ID prefix", () => {
		expect(detectProvider({}, "anthropic.claude-opus-4-6-v1:0:1m")).toBe("amazon-bedrock")
		expect(detectProvider({}, "us.anthropic.claude-sonnet-4-5-v1:0")).toBe("amazon-bedrock")
		expect(detectProvider({}, "eu.anthropic.claude-sonnet-4-5-v1:0")).toBe("amazon-bedrock")
	})
})

describe("suggestSmallModel", () => {
	test("suggests matching provider small model", () => {
		expect(suggestSmallModel("anthropic/claude-opus-4-6")).toBe("anthropic/claude-sonnet-4-5")
		expect(suggestSmallModel("amazon-bedrock/anthropic.claude-opus-4-6")).toContain(
			"amazon-bedrock/",
		)
		expect(suggestSmallModel("google-vertex/claude-opus-4-6")).toContain("google-vertex/")
	})
})

describe("isValidModelId", () => {
	test("valid model IDs", () => {
		expect(isValidModelId("anthropic/claude-opus-4-6")).toBe(true)
		expect(isValidModelId("amazon-bedrock/some-model")).toBe(true)
	})

	test("invalid model IDs", () => {
		expect(isValidModelId("opus")).toBe(false)
		expect(isValidModelId("claude-opus-4-6")).toBe(false)
	})
})
