import { randomBytes } from "node:crypto"
import { createServer } from "node:http"
import { shell } from "electron"
import {
	credentialSetParamsFromDesktopOAuth,
	type DesktopOAuthCredential,
} from "./provider-oauth-credential"

export { credentialSetParamsFromDesktopOAuth, type DesktopOAuthCredential }

export const DESKTOP_OAUTH_PROVIDER_IDS = [
	"openai-codex",
	"anthropic",
	"github-copilot",
	"xai",
] as const

export type DesktopOAuthProviderId = (typeof DESKTOP_OAUTH_PROVIDER_IDS)[number]

export interface DesktopOAuthUpdate {
	url?: string
	instructions: string
	userCode?: string
}

export interface DesktopOAuthOptions {
	signal?: AbortSignal
	enterpriseUrl?: string
	onUpdate: (update: DesktopOAuthUpdate) => void
}

interface DeviceResponse {
	device_code?: unknown
	user_code?: unknown
	verification_uri?: unknown
	verification_uri_complete?: unknown
	interval?: unknown
	expires_in?: unknown
	error?: unknown
	error_description?: unknown
}

const OPENAI_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"
const OPENAI_REDIRECT_URI = "http://localhost:1455/auth/callback"
const ANTHROPIC_CLIENT_ID = Buffer.from(
	"OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl",
	"base64",
).toString()
const ANTHROPIC_REDIRECT_URI = "http://localhost:53692/callback"
const ANTHROPIC_SCOPES =
	"org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
const COPILOT_CLIENT_ID = Buffer.from("SXYxLmI1MDdhMDhjODdlY2ZlOTg=", "base64").toString()
const COPILOT_USER_AGENT = "GitHubCopilotChat/0.48.1"
const XAI_CLIENT_ID = "b1a00492-073a-47ea-816f-4c329264a828"
const XAI_SCOPE = "openid profile email offline_access grok-cli:access api:access"

function requiredString(value: unknown, field: string): string {
	if (typeof value !== "string" || !value.trim()) {
		throw new Error(`OAuth response missing ${field}`)
	}
	return value
}

function requiredNumber(value: unknown, field: string): number {
	if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
		throw new Error(`OAuth response missing ${field}`)
	}
	return value
}

function throwIfCancelled(signal?: AbortSignal): void {
	if (signal?.aborted) throw new Error("Login cancelled")
}

function wait(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise((resolve, reject) => {
		throwIfCancelled(signal)
		const timer = setTimeout(resolve, ms)
		signal?.addEventListener(
			"abort",
			() => {
				clearTimeout(timer)
				reject(new Error("Login cancelled"))
			},
			{ once: true },
		)
	})
}

async function generatePkce(): Promise<{ verifier: string; challenge: string }> {
	const verifier = randomBytes(32).toString("base64url")
	const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))
	return { verifier, challenge: Buffer.from(digest).toString("base64url") }
}

function oauthHtml(title: string, message: string): string {
	return `<!doctype html><html lang="en"><meta charset="utf-8"><title>${title}</title><body style="color-scheme:dark;background:#000;color:#fff;font-family:system-ui;text-align:center;padding:4rem"><h1>${title}</h1><p>${message}</p></body></html>`
}

async function waitForCallback(
	port: number,
	pathname: string,
	expectedState: string,
	signal?: AbortSignal,
): Promise<string> {
	throwIfCancelled(signal)
	return new Promise((resolve, reject) => {
		const server = createServer((request, response) => {
			const url = new URL(request.url ?? "", "http://localhost")
			if (url.pathname !== pathname) {
				response.writeHead(404, { "Content-Type": "text/html; charset=utf-8" })
				response.end(oauthHtml("Authentication failed", "Callback route not found."))
				return
			}
			const state = url.searchParams.get("state")
			const code = url.searchParams.get("code")
			if (state !== expectedState || !code) {
				response.writeHead(400, { "Content-Type": "text/html; charset=utf-8" })
				response.end(oauthHtml("Authentication failed", "Missing code or state mismatch."))
				return
			}
			response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
			response.end(
				oauthHtml("Authentication successful", "Authentication completed. You can close this window."),
			)
			cleanup()
			resolve(code)
		})
		const onAbort = () => {
			cleanup()
			reject(new Error("Login cancelled"))
		}
		const cleanup = () => {
			signal?.removeEventListener("abort", onAbort)
			server.close()
		}
		server.once("error", (error) => {
			cleanup()
			reject(error)
		})
		signal?.addEventListener("abort", onAbort, { once: true })
		server.listen(port, "127.0.0.1")
	})
}

/** Own the localhost callback until browser launch and callback have both settled. */
async function openOAuthCallback(
	port: number,
	pathname: string,
	state: string,
	url: string,
	update: DesktopOAuthUpdate,
	options: DesktopOAuthOptions,
): Promise<string> {
	const controller = new AbortController()
	const onAbort = () => controller.abort()
	if (options.signal?.aborted) controller.abort()
	options.signal?.addEventListener("abort", onAbort, { once: true })
	try {
		const callback = waitForCallback(port, pathname, state, controller.signal)
		// Caller cancellation may reject the callback while openExternal is pending.
		void callback.catch(() => {})
		options.onUpdate(update)
		throwIfCancelled(options.signal)
		await shell.openExternal(url)
		return await callback
	} finally {
		options.signal?.removeEventListener("abort", onAbort)
		controller.abort()
	}
}

async function requestJson(
	url: string,
	init: RequestInit,
	allowHttpError = false,
): Promise<Record<string, unknown>> {
	const response = await fetch(url, init)
	const text = await response.text()
	if (!response.ok && !allowHttpError) {
		throw new Error(`OAuth request failed (${response.status}): ${text || response.statusText}`)
	}
	try {
		const value: unknown = JSON.parse(text)
		if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error()
		return value as Record<string, unknown>
	} catch {
		throw new Error("OAuth request returned invalid JSON")
	}
}

function expiresAt(expiresIn: unknown, skewMs = 0): number {
	return Math.floor((Date.now() + requiredNumber(expiresIn, "expires_in") * 1000 - skewMs) / 1000)
}

async function loginOpenAi(options: DesktopOAuthOptions): Promise<DesktopOAuthCredential> {
	const { verifier, challenge } = await generatePkce()
	throwIfCancelled(options.signal)
	const state = randomBytes(16).toString("hex")
	const url = new URL("https://auth.openai.com/oauth/authorize")
	url.search = new URLSearchParams({
		response_type: "code",
		client_id: OPENAI_CLIENT_ID,
		redirect_uri: OPENAI_REDIRECT_URI,
		scope: "openid profile email offline_access",
		code_challenge: challenge,
		code_challenge_method: "S256",
		state,
		id_token_add_organizations: "true",
		codex_cli_simplified_flow: "true",
		originator: "pi",
	}).toString()
	const code = await openOAuthCallback(
		1455,
		"/auth/callback",
		state,
		url.href,
		{ url: url.href, instructions: "Complete ChatGPT login in your browser." },
		options,
	)
	const body = await requestJson("https://auth.openai.com/oauth/token", {
		method: "POST",
		headers: { "Content-Type": "application/x-www-form-urlencoded" },
		body: new URLSearchParams({
			grant_type: "authorization_code",
			client_id: OPENAI_CLIENT_ID,
			code,
			code_verifier: verifier,
			redirect_uri: OPENAI_REDIRECT_URI,
		}),
		signal: options.signal,
	})
	const access = requiredString(body.access_token, "access_token")
	let accountId: string | undefined
	try {
		const payload = JSON.parse(Buffer.from(access.split(".")[1] ?? "", "base64url").toString())
		accountId = payload?.["https://api.openai.com/auth"]?.chatgpt_account_id
	} catch {
		// The server requires the account id for Codex; report a clear error below.
	}
	if (!accountId) throw new Error("Failed to extract ChatGPT account id")
	return {
		access,
		refresh: requiredString(body.refresh_token, "refresh_token"),
		expiresAt: expiresAt(body.expires_in),
		accountId,
	}
}

async function loginAnthropic(options: DesktopOAuthOptions): Promise<DesktopOAuthCredential> {
	const { verifier, challenge } = await generatePkce()
	throwIfCancelled(options.signal)
	const url = new URL("https://claude.ai/oauth/authorize")
	url.search = new URLSearchParams({
		code: "true",
		client_id: ANTHROPIC_CLIENT_ID,
		response_type: "code",
		redirect_uri: ANTHROPIC_REDIRECT_URI,
		scope: ANTHROPIC_SCOPES,
		code_challenge: challenge,
		code_challenge_method: "S256",
		state: verifier,
	}).toString()
	const code = await openOAuthCallback(
		53692,
		"/callback",
		verifier,
		url.href,
		{ url: url.href, instructions: "Complete Anthropic login in your browser." },
		options,
	)
	const body = await requestJson("https://platform.claude.com/v1/oauth/token", {
		method: "POST",
		headers: { "Content-Type": "application/json", Accept: "application/json" },
		body: JSON.stringify({
			grant_type: "authorization_code",
			client_id: ANTHROPIC_CLIENT_ID,
			code,
			state: verifier,
			redirect_uri: ANTHROPIC_REDIRECT_URI,
			code_verifier: verifier,
		}),
		signal: options.signal,
	})
	return {
		access: requiredString(body.access_token, "access_token"),
		refresh: requiredString(body.refresh_token, "refresh_token"),
		expiresAt: expiresAt(body.expires_in, 5 * 60 * 1000),
	}
}

function normalizeEnterpriseDomain(input?: string): string | undefined {
	const trimmed = input?.trim()
	if (!trimmed) return undefined
	try {
		return new URL(trimmed.includes("://") ? trimmed : `https://${trimmed}`).hostname
	} catch {
		throw new Error("Invalid GitHub Enterprise URL/domain")
	}
}

async function loginCopilot(options: DesktopOAuthOptions): Promise<DesktopOAuthCredential> {
	const enterpriseUrl = normalizeEnterpriseDomain(options.enterpriseUrl)
	const domain = enterpriseUrl ?? "github.com"
	const device = (await requestJson(`https://${domain}/login/device/code`, {
		method: "POST",
		headers: {
			Accept: "application/json",
			"Content-Type": "application/x-www-form-urlencoded",
			"User-Agent": COPILOT_USER_AGENT,
		},
		body: new URLSearchParams({ client_id: COPILOT_CLIENT_ID, scope: "read:user" }),
		signal: options.signal,
	})) as DeviceResponse
	const userCode = requiredString(device.user_code, "user_code")
	const url = requiredString(device.verification_uri, "verification_uri")
	options.onUpdate({ url, userCode, instructions: `Enter code: ${userCode}` })
	await shell.openExternal(url)
	const deadline = Date.now() + requiredNumber(device.expires_in, "expires_in") * 1000
	let interval = Math.max(1000, requiredNumber(device.interval, "interval") * 1000)
	let githubToken: string | undefined
	while (Date.now() < deadline) {
		await wait(Math.min(interval, deadline - Date.now()), options.signal)
		const token = (await requestJson(
			`https://${domain}/login/oauth/access_token`,
			{
				method: "POST",
				headers: {
					Accept: "application/json",
					"Content-Type": "application/x-www-form-urlencoded",
					"User-Agent": COPILOT_USER_AGENT,
				},
				body: new URLSearchParams({
					client_id: COPILOT_CLIENT_ID,
					device_code: requiredString(device.device_code, "device_code"),
					grant_type: "urn:ietf:params:oauth:grant-type:device_code",
				}),
				signal: options.signal,
			},
			true,
		)) as DeviceResponse & { access_token?: unknown }
		if (typeof token.access_token === "string") {
			githubToken = token.access_token
			break
		}
		if (token.error === "authorization_pending") continue
		if (token.error === "slow_down") {
			interval += 5000
			continue
		}
		throw new Error(`GitHub device flow failed: ${String(token.error ?? "invalid response")}`)
	}
	if (!githubToken) throw new Error("GitHub device flow timed out")
	const copilot = await requestJson(`https://api.${domain}/copilot_internal/v2/token`, {
		headers: {
			Accept: "application/json",
			Authorization: `Bearer ${githubToken}`,
			"User-Agent": COPILOT_USER_AGENT,
			"Editor-Version": "vscode/1.136.1",
			"Editor-Plugin-Version": "copilot-chat/0.48.1",
			"Copilot-Integration-Id": "vscode-chat",
		},
		signal: options.signal,
	})
	return {
		access: requiredString(copilot.token, "token"),
		refresh: githubToken,
		expiresAt: requiredNumber(copilot.expires_at, "expires_at") - 5 * 60,
		...(enterpriseUrl ? { enterpriseUrl } : {}),
	}
}

async function loginXai(options: DesktopOAuthOptions): Promise<DesktopOAuthCredential> {
	const device = (await requestJson("https://auth.x.ai/oauth2/device/code", {
		method: "POST",
		headers: { Accept: "application/json", "Content-Type": "application/x-www-form-urlencoded" },
		body: new URLSearchParams({ client_id: XAI_CLIENT_ID, scope: XAI_SCOPE, referrer: "pi" }),
		signal: options.signal,
		redirect: "error",
	})) as DeviceResponse
	const userCode = requiredString(device.user_code, "user_code")
	const url = new URL(requiredString(device.verification_uri, "verification_uri"))
	if (url.protocol !== "https:" || url.username || url.password) {
		throw new Error("Untrusted verification URI in xAI OAuth response")
	}
	options.onUpdate({ url: url.href, userCode, instructions: `Enter code: ${userCode}` })
	await shell.openExternal(url.href)
	const deadline = Date.now() + requiredNumber(device.expires_in, "expires_in") * 1000
	let interval = Math.max(
		1000,
		typeof device.interval === "number" ? device.interval * 1000 : 5000,
	)
	while (Date.now() < deadline) {
		await wait(Math.min(interval, deadline - Date.now()), options.signal)
		const token = (await requestJson(
			"https://auth.x.ai/oauth2/token",
			{
				method: "POST",
				headers: {
					Accept: "application/json",
					"Content-Type": "application/x-www-form-urlencoded",
				},
				body: new URLSearchParams({
					grant_type: "urn:ietf:params:oauth:grant-type:device_code",
					client_id: XAI_CLIENT_ID,
					device_code: requiredString(device.device_code, "device_code"),
				}),
				signal: options.signal,
				redirect: "error",
			},
			true,
		)) as DeviceResponse & {
			access_token?: unknown
			refresh_token?: unknown
		}
		if (typeof token.access_token === "string") {
			const lifetime = requiredNumber(token.expires_in ?? 3600, "expires_in") * 1000
			return {
				access: token.access_token,
				refresh: requiredString(token.refresh_token, "refresh_token"),
				expiresAt: Math.floor(
					(Date.now() + lifetime - Math.min(5 * 60 * 1000, lifetime / 2)) / 1000,
				),
			}
		}
		if (token.error === "authorization_pending") continue
		if (token.error === "slow_down") {
			interval += 5000
			continue
		}
		throw new Error(`xAI device flow failed: ${String(token.error ?? "invalid response")}`)
	}
	throw new Error("xAI device code expired; sign in again")
}

export function isDesktopOAuthProviderId(value: string): value is DesktopOAuthProviderId {
	return DESKTOP_OAUTH_PROVIDER_IDS.some((providerId) => providerId === value)
}

export async function loginDesktopOAuth(
	providerId: DesktopOAuthProviderId,
	options: DesktopOAuthOptions,
): Promise<DesktopOAuthCredential> {
	switch (providerId) {
		case "openai-codex":
			return loginOpenAi(options)
		case "anthropic":
			return loginAnthropic(options)
		case "github-copilot":
			return loginCopilot(options)
		case "xai":
			return loginXai(options)
	}
}
