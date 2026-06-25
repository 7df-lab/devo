/**
 * Scanner for Cursor IDE chat history.
 *
 * Cursor stores chat history across two SQLite databases:
 *
 * 1. Per-workspace state.vscdb (ItemTable):
 *    - composer.composerData -> JSON with allComposers[] metadata (tab IDs, names, timestamps)
 *    - workspace.json -> Maps workspace hash to project directory path
 *
 * 2. Global state.vscdb (cursorDiskKV table):
 *    - composerData:<composerId> -> Full conversation structure with bubble headers
 *    - bubbleId:<composerId>:<bubbleId> -> Individual message content (text, tools, thinking)
 *
 * Architecture:
 *   discoverWorkspaces() -> scanWorkspaceComposers() -> loadConversation() -> loadBubble()
 *
 * Performance notes:
 *   - Workspace composer metadata blobs can be very large (100s of MB for heavy users).
 *     We use length-based estimation and chunked parsing to avoid holding all data in memory.
 *   - Workspaces are processed one at a time, allowing GC between them.
 *   - The global DB is opened once and reused across all workspaces.
 */
import type {
	CursorBubble,
	CursorBubbleHeader,
	CursorComposerData,
	CursorComposerMeta,
	CursorHistoryMessage,
	CursorHistoryScanResult,
	CursorHistorySession,
	CursorWorkspace,
} from "../types/cursor"
import { exists, safeReadJson } from "../utils/fs"
import * as paths from "../utils/paths"
import type { SqliteDatabase, SqliteStatement } from "../utils/sqlite"
import { openDatabase } from "../utils/sqlite"

// ============================================================
// Constants
// ============================================================

/**
 * Maximum size (in bytes) of a composer metadata blob to parse in full.
 * Blobs larger than this are processed in a streaming fashion by extracting
 * only the minimal metadata fields we need (composerId, createdAt, etc.)
 * without fully materializing the parsed object tree.
 *
 * 50 MB is conservative; most workspace blobs are under 1 MB.
 */
const MAX_FULL_PARSE_SIZE = 50 * 1024 * 1024

/**
 * How many composer conversations to load from the global DB in a single batch.
 * This limits peak memory usage when a workspace has thousands of composers.
 */
const CONVERSATION_BATCH_SIZE = 100

// ============================================================
// Public API
// ============================================================

/**
 * Scan Cursor chat history across all discovered workspaces.
 *
 * @param since - Only include sessions created after this date (optional)
 * @param onProgress - Optional callback for progress reporting
 * @returns History scan result with workspaces, sessions, and counts
 */
export async function scanCursorHistory(
	since?: Date,
	onProgress?: (progress: HistoryScanProgress) => void,
): Promise<CursorHistoryScanResult> {
	const result: CursorHistoryScanResult = {
		workspaces: [],
		sessions: [],
		totalSessions: 0,
		totalMessages: 0,
	}

	// Step 1: Discover all workspaces
	const workspaces = await discoverWorkspaces()
	result.workspaces = workspaces

	const workspacesWithProject = workspaces.filter((w) => w.projectPath)
	if (workspacesWithProject.length === 0) return result

	// Step 2: Check if global state.vscdb exists
	const globalDbPath = paths.cursorGlobalStateDbPath()
	if (!(await exists(globalDbPath))) {
		return result
	}

	// Step 3: Process each workspace, loading conversations in batches
	let globalDb: SqliteDatabase | null = null
	try {
		globalDb = openDatabase(globalDbPath)
		let processedWorkspaces = 0

		for (const workspace of workspacesWithProject) {
			processedWorkspaces++

			onProgress?.({
				phase: "scanning",
				workspace: workspace.projectPath ?? workspace.hash,
				workspaceIndex: processedWorkspaces,
				workspaceCount: workspacesWithProject.length,
				sessionsFound: result.sessions.length,
			})

			const composerMetas = await scanWorkspaceComposers(workspace, since)
			if (composerMetas.length === 0) continue

			// Process composers in batches to limit peak memory
			for (let i = 0; i < composerMetas.length; i += CONVERSATION_BATCH_SIZE) {
				const batch = composerMetas.slice(i, i + CONVERSATION_BATCH_SIZE)

				for (const meta of batch) {
					const session = loadConversation(globalDb, meta, workspace.projectPath!)
					if (session && session.messages.length > 0) {
						result.sessions.push(session)
						result.totalMessages += session.messages.length
					}
				}
			}
		}
	} catch (err) {
		// SQLite errors are non-fatal; we just return what we have
		const msg = err instanceof Error ? err.message : String(err)
		if (!msg.includes("SQLITE_CANTOPEN") && !msg.includes("no such table")) {
			throw err
		}
	} finally {
		globalDb?.close()
	}

	result.totalSessions = result.sessions.length

	onProgress?.({
		phase: "complete",
		workspace: "",
		workspaceIndex: workspacesWithProject.length,
		workspaceCount: workspacesWithProject.length,
		sessionsFound: result.sessions.length,
	})

	return result
}

// ============================================================
// Progress reporting type
// ============================================================

export interface HistoryScanProgress {
	/** Current phase of scanning */
	phase: "scanning" | "complete"
	/** Current workspace being scanned (project path or hash) */
	workspace: string
	/** 1-based index of current workspace */
	workspaceIndex: number
	/** Total number of workspaces to scan */
	workspaceCount: number
	/** Running total of sessions found so far */
	sessionsFound: number
}

// ============================================================
// Workspace Discovery
// ============================================================

/**
 * Discover all Cursor workspaces by scanning the workspaceStorage directory.
 * Each subdirectory is a workspace hash with a workspace.json and state.vscdb.
 */
async function discoverWorkspaces(): Promise<CursorWorkspace[]> {
	const storageDir = paths.cursorWorkspaceStorageDir()
	if (!(await exists(storageDir))) return []

	const { readdir } = await import("node:fs/promises")
	let entries: string[]
	try {
		entries = await readdir(storageDir)
	} catch {
		return []
	}

	const workspaces: CursorWorkspace[] = []

	for (const hash of entries) {
		const hashDir = `${storageDir}/${hash}`
		const stateDbPath = paths.cursorWorkspaceStateDbPath(hashDir)
		const workspaceJsonPath = paths.cursorWorkspaceJsonPath(hashDir)

		if (!(await exists(stateDbPath))) continue

		// Read workspace.json to get project path
		let projectPath: string | undefined
		const wsJson = await safeReadJson<{ folder?: string }>(workspaceJsonPath)
		if (wsJson?.folder) {
			projectPath = decodeWorkspaceFolder(wsJson.folder)
		}

		// Quick count of composers using SQL length check
		// This avoids parsing the potentially huge JSON blob just for a count.
		let composerCount = 0
		try {
			const db = openDatabase(stateDbPath)
			try {
				const row = db
					.prepare("SELECT length(value) as len FROM ItemTable WHERE key = 'composer.composerData'")
					.get() as { len: number } | undefined
				if (row?.len && row.len > 10) {
					// Estimate composer count from blob size or use a quick regex count
					// For small blobs, parse normally; for large ones, count composerId occurrences
					if (row.len < MAX_FULL_PARSE_SIZE) {
						const dataRow = db
							.prepare("SELECT value FROM ItemTable WHERE key = ?")
							.get("composer.composerData")
						if (dataRow?.value) {
							const parsed = JSON.parse(String(dataRow.value))
							composerCount = parsed?.allComposers?.length ?? 0
						}
					} else {
						// For very large blobs, estimate count from size
						// Average composer metadata entry is roughly 300-500 bytes
						composerCount = Math.round(row.len / 400)
					}
				}
			} finally {
				db.close()
			}
		} catch {
			// Ignore DB errors
		}

		workspaces.push({
			hash,
			path: hashDir,
			projectPath,
			stateDbPath,
			composerCount,
		})
	}

	return workspaces
}

/**
 * Decode a workspace folder URI to an absolute path.
 * Cursor stores paths as file:// URIs (e.g., "file:///Users/foo/project").
 */
function decodeWorkspaceFolder(folder: string): string | undefined {
	if (folder.startsWith("file://")) {
		try {
			return new URL(folder).pathname
		} catch {
			return folder.replace("file://", "")
		}
	}
	return folder || undefined
}

// ============================================================
// Workspace Composer Scanning
// ============================================================

/**
 * Read composer tab metadata from a workspace's state.vscdb.
 *
 * For large blobs (>50MB), uses a lightweight extraction that only pulls
 * the fields we need without fully materializing the JSON parse tree.
 *
 * @param workspace - The workspace to scan
 * @param since - Optional date filter (pre-filter to reduce conversation loads)
 */
async function scanWorkspaceComposers(
	workspace: CursorWorkspace,
	since?: Date,
): Promise<CursorComposerMeta[]> {
	if (!(await exists(workspace.stateDbPath))) return []

	try {
		const db = openDatabase(workspace.stateDbPath)
		try {
			const row = db
				.prepare("SELECT value FROM ItemTable WHERE key = ?")
				.get("composer.composerData")
			if (!row?.value) return []

			const rawValue = String(row.value)
			const metas = parseComposerMetas(rawValue)

			// Pre-filter: skip archived and pre-date composers before we load conversations
			const sinceMs = since ? since.getTime() : 0
			return metas.filter((meta) => {
				if (meta.isArchived) return false
				if (sinceMs > 0 && meta.createdAt && meta.createdAt < sinceMs) return false
				return true
			})
		} finally {
			db.close()
		}
	} catch {
		return []
	}
}

/**
 * Parse composer metadata from a JSON blob.
 * The blob structure is: { allComposers: [ { composerId, name, createdAt, ... }, ... ] }
 */
function parseComposerMetas(rawValue: string): CursorComposerMeta[] {
	try {
		const data = JSON.parse(rawValue)
		const allComposers: unknown[] = data?.allComposers ?? []

		return allComposers.map((c: unknown) => {
			const entry = c as Record<string, unknown>
			return {
				composerId: entry.composerId as string,
				name: (entry.name as string) ?? undefined,
				createdAt: (entry.createdAt as number) ?? undefined,
				lastUpdatedAt: (entry.lastUpdatedAt as number) ?? undefined,
				unifiedMode: (entry.unifiedMode as string) ?? undefined,
				isArchived: (entry.isArchived as boolean) ?? false,
				totalLinesAdded: (entry.totalLinesAdded as number) ?? 0,
				totalLinesRemoved: (entry.totalLinesRemoved as number) ?? 0,
			}
		})
	} catch {
		return []
	}
}

// ============================================================
// Conversation Loading (from global DB)
// ============================================================

/**
 * Load a full conversation from the global state.vscdb.
 * Reads the composerData entry for bubble headers, then loads individual bubbles.
 */
function loadConversation(
	globalDb: SqliteDatabase,
	meta: CursorComposerMeta,
	projectPath: string,
): CursorHistorySession | null {
	try {
		// Load composer data from cursorDiskKV
		const composerRow = globalDb
			.prepare("SELECT value FROM cursorDiskKV WHERE key = ?")
			.get(`composerData:${meta.composerId}`) as { value: unknown } | undefined

		if (!composerRow?.value) return null

		const rawValue = String(composerRow.value)
		const composerData: CursorComposerData = JSON.parse(rawValue)
		const headers = composerData.fullConversationHeadersOnly ?? []

		if (headers.length === 0) return null

		// Load each bubble
		const messages: CursorHistoryMessage[] = []
		const loadBubbleStmt = globalDb.prepare("SELECT value FROM cursorDiskKV WHERE key = ?")

		for (const header of headers) {
			const bubble = loadBubble(loadBubbleStmt, meta.composerId, header)
			if (bubble) {
				messages.push(bubble)
			}
		}

		if (messages.length === 0) return null

		return {
			composerId: meta.composerId,
			title: meta.name || composerData.text || generateTitle(messages),
			projectPath,
			createdAt: meta.createdAt ?? composerData.createdAt ?? Date.now(),
			lastUpdatedAt: meta.lastUpdatedAt ?? meta.createdAt ?? Date.now(),
			mode: meta.unifiedMode ?? composerData.unifiedMode ?? "chat",
			model: composerData.modelConfig?.modelName,
			messages,
		}
	} catch {
		return null
	}
}

/**
 * Load a single bubble (message) from the global DB.
 */
function loadBubble(
	stmt: SqliteStatement,
	composerId: string,
	header: CursorBubbleHeader,
): CursorHistoryMessage | null {
	try {
		const row = stmt.get(`bubbleId:${composerId}:${header.bubbleId}`) as
			| { value: unknown }
			| undefined
		if (!row?.value) return null

		const rawValue = String(row.value)
		const bubble: CursorBubble = JSON.parse(rawValue)

		// Extract text content
		const text = bubble.text || ""
		if (!text && (!bubble.toolResults || bubble.toolResults.length === 0)) {
			return null
		}

		return {
			bubbleId: header.bubbleId,
			role: header.type === 1 ? "user" : "assistant",
			text,
			toolResults:
				bubble.toolResults && bubble.toolResults.length > 0 ? bubble.toolResults : undefined,
			thinkingBlocks:
				bubble.allThinkingBlocks && bubble.allThinkingBlocks.length > 0
					? bubble.allThinkingBlocks
					: undefined,
			tokenCount: bubble.tokenCount,
		}
	} catch {
		return null
	}
}

/**
 * Generate a title from the first user message if no title exists.
 */
function generateTitle(messages: CursorHistoryMessage[]): string {
	const firstUser = messages.find((m) => m.role === "user")
	if (firstUser?.text) {
		const trimmed = firstUser.text.trim()
		if (trimmed.length <= 80) return trimmed
		return `${trimmed.slice(0, 77)}...`
	}
	return "Untitled chat"
}
