import { describe, expect, test } from "bun:test"
import { credentialSetParamsFromDesktopOAuth } from "./provider-oauth-credential"

describe("credentialSetParamsFromDesktopOAuth", () => {
	/**
	 * Trace: L2-DES-AUTH-001
	 * Verifies: Desktop OAuth login persists via Native credential/set oauth params.
	 */
	test("maps OAuth credential into credential/set params", () => {
		expect(
			credentialSetParamsFromDesktopOAuth("openai-codex", {
				access: "access-token",
				refresh: "refresh-token",
				expiresAt: 1_700_000_000_000,
				accountId: "acct_1",
			}),
		).toEqual({
			provider: "openai-codex",
			kind: "oauth",
			access: "access-token",
			refresh: "refresh-token",
			expiresAt: 1_700_000_000_000,
			accountId: "acct_1",
		})
	})
})
