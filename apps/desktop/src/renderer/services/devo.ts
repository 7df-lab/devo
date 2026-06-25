import type { DevoClient } from "@devo-ai/sdk/v2/client"
import { createDevoClient } from "@devo-ai/sdk/v2/client"
import type { Event, DevoProject, QuestionAnswer, Session, SessionStatus } from "../lib/types"
import { createLogger } from "../lib/logger"

export type { DevoClient }

const log = createLogger("devo-service")

/**
 * Build an HTTP Basic Auth header value from username and password.
 */
export function buildBasicAuthHeader(username: string, password: string): string {
	return `Basic ${btoa(`${username}:${password}`)}`
}

// ============================================================
// Client creation
// ============================================================

export interface ConnectOptions {
	/** Project directory for scoped requests. */
	directory?: string
	/** Pre-built Authorization header value (e.g. "Basic dXNlcjpwYXNz"). */
	authHeader?: string
}

/**
 * Creates a Devo client over the preload ACP bridge.
 */
export function connectToServer(url: string, options?: ConnectOptions): DevoClient {
	void url
	void options?.authHeader
	return createDevoClient({ directory: options?.directory })
}

/**
 * Fetch all projects known to the server.
 */
export async function listProjects(client: DevoClient): Promise<DevoProject[]> {
	const result = await client.project.list()
	return (result.data as DevoProject[]) ?? []
}

/**
 * Fetch sessions from a server with optional filtering/pagination.
 *
 * @param limit  Maximum number of sessions to return (server default: 100)
 * @param roots  Only return root sessions (no sub-agents)
 * @param search Filter sessions by title (case-insensitive substring match)
 */
export async function listSessions(
	client: DevoClient,
	options?: { limit?: number; roots?: boolean; search?: string },
): Promise<Session[]> {
	const params = {
		limit: options?.limit,
		roots: options?.roots,
		search: options?.search,
	}
	log.info("Listing sessions", params)
	const result = await client.session.list(params)
	const sessions = (result.data as Session[]) ?? []
	log.info("Listed sessions", { count: sessions.length, ...params })
	return sessions
}

/**
 * Get session statuses (running/idle/retry) for all sessions.
 */
export async function getSessionStatuses(
	client: DevoClient,
): Promise<Record<string, SessionStatus>> {
	const result = await client.session.status()
	return (result.data as Record<string, SessionStatus>) ?? {}
}

/**
 * Create a new session (= new agent).
 */
export async function createSession(client: DevoClient, title?: string): Promise<Session> {
	const result = await client.session.create({ title })
	return result.data as Session
}

/**
 * Send a prompt to a session (async — returns immediately, track via events).
 */
export async function sendPrompt(
	client: DevoClient,
	sessionId: string,
	text: string,
	options?: {
		providerID?: string
		modelID?: string
		agent?: string
		variant?: string
	},
): Promise<void> {
	await client.session.promptAsync({
		sessionID: sessionId,
		parts: [{ type: "text", text }],
		model:
			options?.providerID && options?.modelID
				? { providerID: options.providerID, modelID: options.modelID }
				: undefined,
		agent: options?.agent,
		variant: options?.variant,
	})
}

/**
 * Abort a running session.
 */
export async function abortSession(client: DevoClient, sessionId: string): Promise<void> {
	await client.session.abort({ sessionID: sessionId })
}

/**
 * Rename a session (update its title).
 */
export async function renameSession(
	client: DevoClient,
	sessionId: string,
	title: string,
): Promise<void> {
	await client.session.update({ sessionID: sessionId, title })
}

/**
 * Delete a session.
 */
export async function deleteSession(client: DevoClient, sessionId: string): Promise<void> {
	await client.session.delete({ sessionID: sessionId })
}

/**
 * Fetch a single session by ID.
 * Returns null if the session is not found or the request fails.
 */
export async function getSession(client: DevoClient, sessionId: string): Promise<Session | null> {
	try {
		const result = await client.session.get({ sessionID: sessionId })
		return (result.data as Session) ?? null
	} catch {
		return null
	}
}

/**
 * Get file diffs for a session.
 */
export async function getSessionDiff(client: DevoClient, sessionId: string) {
	const result = await client.session.diff({ sessionID: sessionId })
	return result.data ?? []
}

/**
 * Respond to a permission request.
 */
export async function respondToPermission(
	client: DevoClient,
	sessionId: string,
	permissionId: string,
	response: "once" | "always" | "reject",
): Promise<void> {
	await client.permission.respond({
		sessionID: sessionId,
		permissionID: permissionId,
		response,
	})
}

/**
 * Reply to a question request from the AI assistant.
 */
export async function replyToQuestion(
	client: DevoClient,
	requestId: string,
	answers: QuestionAnswer[],
): Promise<void> {
	await client.question.reply({ requestID: requestId, answers })
}

/**
 * Reject a question request from the AI assistant.
 */
export async function rejectQuestion(client: DevoClient, requestId: string): Promise<void> {
	await client.question.reject({ requestID: requestId })
}

/**
 * Dispose a specific project instance on the Devo server.
 * This forces the server to re-read all config, agents, skills, etc. from disk
 * for that project. The resulting `server.instance.disposed` ACP event triggers
 * automatic query invalidation in the UI.
 */
export async function disposeInstance(client: DevoClient): Promise<void> {
	await client.instance.dispose()
}

/**
 * Dispose all instances on the Devo server (global reload).
 * Forces re-initialization of all project instances, re-reading all config
 * files, agents, skills, commands, etc. from disk. The resulting
 * `global.disposed` ACP event triggers automatic query invalidation in the UI.
 */
export async function disposeAllInstances(client: DevoClient): Promise<void> {
	await client.global.dispose()
}

/**
 * Global event from the /global/event ACP event stream.
 * Wraps each Event with the directory it belongs to.
 */
export interface GlobalEvent {
	directory: string
	payload: Event
}

/**
 * Subscribe to global ACP events from the server.
 * Uses `/global/event` which streams events from ALL projects,
 * each tagged with their directory. This avoids the per-directory
 * scoping issue where `/event` only returns events for one Instance.
 */
export async function subscribeToGlobalEvents(
	client: DevoClient,
): Promise<AsyncIterable<GlobalEvent>> {
	const result = await client.global.event()
	return result.stream as AsyncIterable<GlobalEvent>
}

/**
 * Revert a session to a specific message (undo).
 * Rolls back filesystem changes and marks messages after the revert point.
 */
export async function revertSession(
	client: DevoClient,
	sessionId: string,
	messageId: string,
): Promise<Session> {
	const result = await client.session.revert({
		sessionID: sessionId,
		messageID: messageId,
	})
	return result.data as Session
}

/**
 * Unrevert a session (redo).
 * Restores previously reverted messages and filesystem state.
 */
export async function unrevertSession(client: DevoClient, sessionId: string): Promise<Session> {
	const result = await client.session.unrevert({
		sessionID: sessionId,
	})
	return result.data as Session
}

/**
 * Execute a named command on a session.
 * Server-side commands like /init, /review, or user-defined commands.
 */
export async function executeCommand(
	client: DevoClient,
	sessionId: string,
	command: string,
	args: string,
): Promise<void> {
	await client.session.command({
		sessionID: sessionId,
		command,
		arguments: args,
	})
}

/**
 * List available commands from the server.
 */
export async function listCommands(
	client: DevoClient,
): Promise<Array<{ name: string; description?: string }>> {
	const result = await client.command.list()
	return (result.data ?? []) as Array<{ name: string; description?: string }>
}

/**
 * Search for files in the project.
 * Returns file paths as strings (from the Devo /find/file endpoint).
 */
export async function findFiles(client: DevoClient, query: string): Promise<string[]> {
	const result = await client.find.files({ query })
	return (result.data ?? []) as string[]
}

/**
 * Fork a session, optionally at a specific message boundary.
 * Copies all messages up to (but not including) the given messageId.
 * If no messageId is provided, copies the entire conversation.
 */
export async function forkSession(
	client: DevoClient,
	sessionId: string,
	messageId?: string,
): Promise<Session> {
	const result = await client.session.fork({
		sessionID: sessionId,
		messageID: messageId,
	})
	return result.data as Session
}

/**
 * Summarize/compact a session conversation.
 */
export async function summarizeSession(client: DevoClient, sessionId: string): Promise<void> {
	await client.session.summarize({ sessionID: sessionId })
}

/**
 * Get messages for a session (for initial load of activity feed).
 */
export async function getSessionMessages(client: DevoClient, sessionId: string) {
	const result = await client.session.messages({
		sessionID: sessionId,
	})
	return result.data ?? []
}
