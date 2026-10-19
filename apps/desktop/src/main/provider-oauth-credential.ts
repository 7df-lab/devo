export interface DesktopOAuthCredential {
	access: string
	refresh?: string
	expiresAt?: number
	accountId?: string
	enterpriseUrl?: string
}

/** Native `credential/set` params after a successful Desktop OAuth login. */
export function credentialSetParamsFromDesktopOAuth(
	providerId: string,
	credential: DesktopOAuthCredential,
): {
	provider: string
	kind: "oauth"
	access: string
	refresh?: string
	expiresAt?: number
	accountId?: string
	enterpriseUrl?: string
} {
	return {
		provider: providerId,
		kind: "oauth",
		access: credential.access,
		...(credential.refresh ? { refresh: credential.refresh } : {}),
		...(credential.expiresAt != null ? { expiresAt: credential.expiresAt } : {}),
		...(credential.accountId ? { accountId: credential.accountId } : {}),
		...(credential.enterpriseUrl ? { enterpriseUrl: credential.enterpriseUrl } : {}),
	}
}
