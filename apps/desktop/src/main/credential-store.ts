/**
 * Encrypted credential storage backed by Electron's safeStorage API.
 *
 * Passwords are encrypted with the OS keychain (macOS Keychain, Windows DPAPI,
 * Linux libsecret) and stored in a JSON file in the app's userData directory.
 * The renderer never sees the raw file; it communicates via IPC.
 */

import fs from "node:fs"
import path from "node:path"
import { app, safeStorage } from "electron"
import { createLogger } from "./logger"

const log = createLogger("credential-store")

// ============================================================
// State
// ============================================================

/** Map of serverId -> base64-encoded encrypted password */
let credentials: Record<string, string> = {}
let credentialsPath: string | null = null

// ============================================================
// Public API
// ============================================================

/** Initialize the credential store. Call once from app.whenReady(). */
export function initCredentialStore(): void {
	credentialsPath = path.join(app.getPath("userData"), "credentials.json")

	try {
		if (fs.existsSync(credentialsPath)) {
			const raw = fs.readFileSync(credentialsPath, "utf-8")
			credentials = JSON.parse(raw)
			log.info("Credentials loaded", {
				path: credentialsPath,
				count: Object.keys(credentials).length,
			})
		} else {
			log.info("No credentials file found, starting empty", { path: credentialsPath })
		}
	} catch (err) {
		log.error("Failed to load credentials, starting empty", err)
		credentials = {}
	}
}

/**
 * Store a password for a server, encrypted via the OS keychain.
 * Overwrites any existing password for the same serverId.
 */
export function storeCredential(serverId: string, password: string): void {
	if (!safeStorage.isEncryptionAvailable()) {
		log.warn("safeStorage encryption not available, storing in plain text fallback")
		credentials[serverId] = Buffer.from(password, "utf-8").toString("base64")
	} else {
		const encrypted = safeStorage.encryptString(password)
		credentials[serverId] = encrypted.toString("base64")
	}
	persist()
	log.info("Credential stored", { serverId })
}

/**
 * Retrieve a decrypted password for a server.
 * Returns null if no password is stored for the given serverId.
 */
export function getCredential(serverId: string): string | null {
	const encoded = credentials[serverId]
	if (!encoded) return null

	try {
		if (!safeStorage.isEncryptionAvailable()) {
			return Buffer.from(encoded, "base64").toString("utf-8")
		}
		const encrypted = Buffer.from(encoded, "base64")
		return safeStorage.decryptString(encrypted)
	} catch (err) {
		log.error("Failed to decrypt credential", { serverId }, err)
		return null
	}
}

/** Delete a stored credential. */
export function deleteCredential(serverId: string): void {
	if (!(serverId in credentials)) return
	delete credentials[serverId]
	persist()
	log.info("Credential deleted", { serverId })
}

// ============================================================
// Internal
// ============================================================

function persist(): void {
	if (!credentialsPath) return
	try {
		const dir = path.dirname(credentialsPath)
		if (!fs.existsSync(dir)) {
			fs.mkdirSync(dir, { recursive: true })
		}
		const tmpPath = `${credentialsPath}.tmp`
		fs.writeFileSync(tmpPath, JSON.stringify(credentials, null, "\t"), "utf-8")
		fs.renameSync(tmpPath, credentialsPath)
		log.debug("Credentials persisted", { path: credentialsPath })
	} catch (err) {
		log.error("Failed to persist credentials", err)
	}
}
