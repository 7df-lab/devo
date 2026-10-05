import { describe, expect, test } from "bun:test"
import {
	isBuiltinTemplateProviderId,
	suggestNonTemplateProviderId,
} from "./custom-provider-id"

describe("custom-provider-id", () => {
	test("detects builtin template collisions", () => {
		const templates = new Set(["deepseek", "openai"])
		expect(isBuiltinTemplateProviderId("deepseek", templates)).toBe(true)
		expect(isBuiltinTemplateProviderId("  deepseek  ", templates)).toBe(true)
		expect(isBuiltinTemplateProviderId("my-deepseek", templates)).toBe(false)
		expect(isBuiltinTemplateProviderId("", templates)).toBe(false)
	})

	test("suggests a free custom-suffixed id", () => {
		const templates = new Set(["deepseek", "deepseek-custom"])
		expect(suggestNonTemplateProviderId("deepseek", templates)).toBe(
			"deepseek-custom-2",
		)
		expect(suggestNonTemplateProviderId("openai", new Set(["openai"]))).toBe(
			"openai-custom",
		)
	})
})
