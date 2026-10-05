import { describe, expect, test } from "bun:test"
import {
	resolveProviderAuthMethods,
	settingsConnectDialogKind,
} from "./desktop-oauth-providers"

describe("resolveProviderAuthMethods", () => {
	/**
	 * Trace: L2-DES-AUTH-001
	 * Verifies: Desktop exposes the four supported provider OAuth logins.
	 */
	test("provides the native OAuth providers", () => {
		expect(
			Object.fromEntries(
				["openai-codex", "anthropic", "github-copilot", "xai"].map((providerId) => [
					providerId,
					resolveProviderAuthMethods(providerId),
				]),
			),
		).toEqual({
			"openai-codex": [{ type: "oauth", label: "Sign in with ChatGPT" }],
			anthropic: [
				{ type: "oauth", label: "Sign in with Claude Pro/Max" },
				{ type: "api", label: "API Key" },
			],
			"github-copilot": [{ type: "oauth", label: "Sign in with GitHub Copilot" }],
			xai: [
				{ type: "oauth", label: "Sign in with xAI" },
				{ type: "api", label: "API Key" },
			],
		})
	})

	/**
	 * Trace: L2-DES-AUTH-001
	 * Verifies: Settings Connect routes OAuth builtins to the OAuth dialog.
	 */
	test("settingsConnectDialogKind routes OAuth vs API-key templates", () => {
		expect(settingsConnectDialogKind("openai-codex")).toBe("oauth")
		expect(settingsConnectDialogKind("anthropic")).toBe("oauth")
		expect(settingsConnectDialogKind("github-copilot")).toBe("oauth")
		expect(settingsConnectDialogKind("xai")).toBe("oauth")
		expect(settingsConnectDialogKind("openai")).toBe("api-key-template")
		expect(settingsConnectDialogKind("deepseek")).toBe("api-key-template")
	})
})
