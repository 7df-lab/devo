// @ts-nocheck

import {
	AsyncEventQueue,
	type SessionConfigOption,
	configDataFromConfigOptions,
	createIpcTransport,
	defaultCwd,
	permissionOptionId,
	providerDataFromConfigOptions,
	questionInfoFromNative,
	sessionErrorEvent,
	stableId,
} from "./native-client-support"
import {
	type NativeItemEnvelope,
	envelopeFromWire,
	mergeNativeEnvelope,
	nativeItemType,
	recentNativeItems,
} from "./native-item"
export type { NativeItemEnvelope } from "./native-item"
export {
	assistantOrReasoningText,
	compareNativeItems,
	envelopeFromWire,
	isUserMessageItem,
	mergeNativeEnvelope,
	nativeItemType,
	recentNativeItems,
	sortedNativeItems,
	userMessageText,
} from "./native-item"
import type {
	ModelListResult,
	CredentialListResult,
	ModelCatalogRefreshParams,
	ModelCatalogRefreshResult,
	ProviderDisconnectParams,
	ProviderDisconnectResult,
	ProviderDiscoverParams,
	ProviderDiscoverResult,
	ProviderInfo,
	ProviderListResult,
	ProviderModelInfo,
	ProviderModelRemoveParams,
	ProviderModelRemoveResult,
	ProviderModelVariant,
	ProviderUpsertParams,
	ProviderUpsertResult,
	ProviderValidateParams,
	ProviderValidateResult,
	InputModality,
	UserInput,
	TurnStartResult,
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeViewStatus,
	WorkspaceChangesReadParams,
	WorkspaceChangesReadResult,
	WorkspaceDiffDetail,
	SessionInterruptParams,
} from "./generated/native"
import {
	ProtocolValidationError,
	assertValidProtocolPayload,
	dropUnknownReplayEnvelopes,
} from "./protocol-validation"
import {
	ReferenceSearchSession,
	type ReferenceSearchSnapshot,
} from "./reference-search-session"

export type {
	ReferenceSearchResult,
	ReferenceSearchSnapshot,
} from "./reference-search-session"

export type JsonRpcId = number | string

export interface DevoNativeTransportEvent {
	type: "notification" | "request" | "closed"
	id?: JsonRpcId
	method?: string
	params?: unknown
	error?: string
}

export interface DevoNativeTransport {
	request(method: string, params?: unknown, directory?: string): Promise<unknown>
	notify?(method: string, params?: unknown, directory?: string): Promise<void>
	respond(id: JsonRpcId, result: unknown): Promise<void>
	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void
	connected(): boolean
}

export interface CreateDevoClientOptions {
	baseUrl?: string
	directory?: string
	fetch?: typeof fetch
	transport?: DevoNativeTransport
}

export type Agent = any
export type AgentConfig = any
export type AgentPart = any
export type AssistantMessage = any
export type Command = any
export type CompactionPart = any
export type Config = any
export type Event = any
export type EventMessagePartDelta = any
export type EventMessagePartUpdated = any
export type EventPermissionAsked = any
export type EventSessionCreated = any
export type EventSessionDeleted = any
export type EventSessionError = any
export type EventSessionStatus = any
export type EventSessionUpdated = any
export type FileDiff = any
export type FilePart = any
export type FilePartInput = any
export type McpLocalConfig = any
export type McpOAuthConfig = any
export type McpRemoteConfig = any
export type Message = any
export type Model = any
export type Part = any
export type PatchPart = any
export type PermissionAction = any
export type PermissionActionConfig = any
export type PermissionConfig = any
export type PermissionObjectConfig = any
export type PermissionRequest = any
export type PermissionResponse =
	| "once"
	| "turn"
	| "session"
	| "pathPrefix"
	| "host"
	| "tool"
	| "commandPrefix"
	| "commandPrefixPersist"
	| "always"
	| "reject"
export type PermissionRule = any
export type PermissionRuleConfig = any
export type PermissionRuleset = any
export type Project = any
export type Provider = any
export type ProviderAuthMethod = any
export type ProviderConfig = any
export type QuestionOption = {
	label: string
	description: string
}

export type QuestionInfo = {
	id: string
	header: string
	question: string
	options: QuestionOption[]
	isOther: boolean
	isSecret: boolean
}

export type QuestionRequest = {
	id: string
	sessionId: string
	questions: QuestionInfo[]
}

export type QuestionAnswer = string[]
export type ReasoningPart = any
export type RetryPart = any
export type ServerConfig = any
export type Session = any
export type SessionStatus = any
export type SnapshotPart = any
export type StepFinishPart = any
export type StepStartPart = any
export type SubtaskPart = any
export type TextPart = any
export type Todo = any
export type ToolPart = any
export type ToolState = any
export type ToolStateCompleted = any
export type UserMessage = any
export type Worktree = any
export type {
	ModelInfo,
	CredentialInfo,
	ModelListResult,
	CredentialListResult,
	ModelCatalogRefreshParams,
	ModelCatalogRefreshResult,
	ProviderDisconnectParams,
	ProviderDisconnectResult,
	ProviderDiscoverParams,
	ProviderDiscoverResult,
	ProviderInfo,
	ProviderListResult,
	ProviderModelInfo,
	ProviderModelRemoveParams,
	ProviderModelRemoveResult,
	ProviderModelVariant,
	ProviderUpsertParams,
	ProviderUpsertResult,
	ProviderValidateParams,
	ProviderValidateResult,
	ProviderWireApi,
	InputModality,
	ReasoningCapability,
	ReasoningEffort,
	ReasoningLevelChoice,
	WorkspaceChangeAttribution,
	WorkspaceChangeBase,
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeStats,
	WorkspaceChangeView,
	WorkspaceChangeViewStatus,
	WorkspaceChangedFile,
	WorkspaceChangedFileStatus,
	WorkspaceChangesReadParams,
	WorkspaceChangesReadResult,
	WorkspaceDiffDetail,
} from "./generated/native"

/** Native `workspace/changes/updated` notification params (wire camelCase). */
type WorkspaceChangesUpdatedNotification = {
	sessionId: string
	turnId: string
	scope: WorkspaceChangeScope
	status: WorkspaceChangeViewStatus
	coverage: WorkspaceChangeCoverage
	changeSetStatus: WorkspaceChangeSetStatus
	stats: { filesChanged: number; additions: number; deletions: number }
	version: number
	generatedAt: string
}

// ── Canonical provider/model catalog types (L2-DES-MODEL-002) ──

/** Canonical provider/model types generated from the Native protocol schema. */
export type CatalogWireApi = ProviderWireApi
export type CatalogModelVariant = ProviderModelVariant
export type CatalogModelInfo = ProviderModelInfo
export type CatalogProviderInfo = ProviderInfo
export type ProviderCatalogListResult = ProviderListResult
export type CatalogProviderUpsertParams = ProviderUpsertParams
export type CatalogProviderUpsertResult = ProviderUpsertResult
export type CatalogProviderDisconnectParams = ProviderDisconnectParams
export type CatalogProviderDisconnectResult = ProviderDisconnectResult
export type CatalogProviderModelRemoveParams = ProviderModelRemoveParams
export type CatalogProviderModelRemoveResult = ProviderModelRemoveResult
export type CatalogProviderValidateParams = ProviderValidateParams
export type CatalogProviderValidateResult = ProviderValidateResult
export type CatalogProviderDiscoverParams = ProviderDiscoverParams
export type CatalogProviderDiscoverResult = ProviderDiscoverResult
/**
 * First-party workspace/changes/read options — Native wire names (`sessionId`,
 * `turnId`, …). Chat item view models and permission/question events use the
 * same camelCase identity fields (`sessionId`/`itemId`/`turnId`/`requestId`).
 */
export type WorkspaceChangesReadOptions = Omit<WorkspaceChangesReadParams, "diffDetail"> & {
	/** Optional; defaults to summary when omitted. */
	diffDetail?: WorkspaceDiffDetail
}

/** Native `workspace/changes/updated` event properties (wire camelCase). */
export type WorkspaceChangesUpdatedEventProperties = WorkspaceChangesUpdatedNotification

interface GlobalEvent {
	directory: string
	payload: Event
}

type PendingQuestion = {
	id?: JsonRpcId
	method?: string
	sessionId: string
	questions: QuestionInfo[]
}

type PendingPermission = {
	id?: JsonRpcId
	method: string
	sessionId?: string
	options: Array<{ optionId: string; kind: string }>
	availableScopes?: string[]
	native?: boolean
}

function objectRecord(value: unknown): Record<string, unknown> | undefined {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : undefined
}

/** Canonical Native plan-entry statuses — no snake_case reshape. */
function planStatus(status: string): string {
	switch (status) {
		case "completed":
		case "inProgress":
		case "cancelled":
		case "pending":
			return status
		default:
			return "pending"
	}
}

type MappedPlanEntry = { content: string; status: string }

function mapPlanEntry(entry: unknown): MappedPlanEntry | null {
	const value = objectRecord(entry) ?? {}
	const content = String(value.step ?? value.content ?? value.title ?? "").trim()
	if (!content) return null
	return {
		content,
		status: planStatus(String(value.status ?? "pending")),
	}
}

/**
 * Expand a single Plan entry when the server historically stored the whole
 * `update_plan` output as one `step` (JSON object, JSON array, or Mixed text
 * with a trailing pretty-printed plan array). Structured entries pass through.
 */
function expandPlanEntries(entries: unknown[]): MappedPlanEntry[] {
	const mapped = entries.map(mapPlanEntry).filter((entry): entry is MappedPlanEntry => entry !== null)
	if (mapped.length !== 1) return mapped
	const only = mapped[0]
	const expanded = expandPlanEntriesFromBlob(only.content)
	return expanded.length > 0 ? expanded : mapped
}

function expandPlanEntriesFromBlob(content: string): MappedPlanEntry[] {
	const trimmed = content.trim()
	if (!trimmed) return []

	const tryParsePlan = (value: unknown): MappedPlanEntry[] => {
		const plan = Array.isArray(value)
			? value
			: Array.isArray(objectRecord(value)?.plan)
				? (objectRecord(value)?.plan as unknown[])
				: null
		if (!plan) return []
		return plan.map(mapPlanEntry).filter((entry): entry is MappedPlanEntry => entry !== null)
	}

	if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
		try {
			return tryParsePlan(JSON.parse(trimmed) as unknown)
		} catch {
			// Fall through to Mixed-text extraction.
		}
	}

	// Mixed tool text: "<explanation>\n\n[ { status, step }, ... ]"
	const arrayStart = trimmed.indexOf("\n[")
	if (arrayStart >= 0) {
		const maybeArray = trimmed.slice(arrayStart + 1).trim()
		if (maybeArray.startsWith("[")) {
			try {
				return tryParsePlan(JSON.parse(maybeArray) as unknown)
			} catch {
				return []
			}
		}
	}

	// Embedded update_plan-shaped objects inside prose.
	if (
		/"status"\s*:\s*"(?:pending|completed|inProgress|cancelled)"/.test(trimmed) &&
		/"(?:step|content)"\s*:/.test(trimmed)
	) {
		const objectStart = trimmed.indexOf("{")
		const arrayStartInline = trimmed.indexOf("[")
		const start =
			objectStart >= 0 && (arrayStartInline < 0 || objectStart < arrayStartInline)
				? objectStart
				: arrayStartInline
		if (start >= 0) {
			try {
				return tryParsePlan(JSON.parse(trimmed.slice(start)) as unknown)
			} catch {
				return []
			}
		}
	}

	return []
}

const KNOWN_PERMISSION_SCOPES: PermissionResponse[] = [
	"once",
	"turn",
	"session",
	"pathPrefix",
	"host",
	"tool",
	"commandPrefix",
	"commandPrefixPersist",
]

/** Drop unknown tokens; keep only canonical Native approval scopes (no snake_case aliases). */
function knownApprovalScopes(scopes: string[] | undefined): PermissionResponse[] {
	const normalized: PermissionResponse[] = []
	const seen = new Set<string>()
	for (const scope of scopes ?? ["once"]) {
		if ((KNOWN_PERMISSION_SCOPES as string[]).includes(scope) && !seen.has(scope)) {
			seen.add(scope)
			normalized.push(scope as PermissionResponse)
		}
	}
	return normalized.length > 0 ? normalized : ["once"]
}

function approvalMethodFromResource(resource: unknown): string {
	const resourceText = String(resource ?? "").toLowerCase()
	if (resourceText.includes("filewrite") || resourceText.includes("file_write")) {
		return "approval/fileChange/request"
	}
	if (
		resourceText.includes("shellexec") ||
		resourceText.includes("shell") ||
		resourceText.includes("command") ||
		resourceText.includes("process")
	) {
		return "approval/command/request"
	}
	return "approval/permission/request"
}

function nativeItemNotificationMethod(envelope: Record<string, unknown>): "item/started" | "item/completed" {
	const state = String(envelope.state ?? "")
	if (state === "completed" || state === "failed" || state === "interrupted") return "item/completed"
	return "item/started"
}

function stringOrUndefined(value: unknown): string | undefined {
	return typeof value === "string" && value.length > 0 ? value : undefined
}

function numberFromProtocol(value: unknown): number {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value === "bigint") return Number(value)
	if (typeof value === "string") {
		const parsed = Number(value)
		if (Number.isFinite(parsed)) return parsed
	}
	return 0
}

type ContextOccupancyWire = {
	totalTokens: number
	contextWindowTokens: number
	categories: Array<{ id: string; tokens: number; shareBps: number }>
}

function contextOccupancyFromProtocol(value: unknown): ContextOccupancyWire | null {
	const occupancy = objectRecord(value)
	if (!occupancy) return null
	const rawCategories = Array.isArray(occupancy.categories) ? occupancy.categories : []
	return {
		totalTokens: numberFromProtocol(occupancy.totalTokens),
		contextWindowTokens: numberFromProtocol(occupancy.contextWindowTokens),
		categories: rawCategories.flatMap((entry) => {
			const category = objectRecord(entry)
			const id = String(category?.id ?? "")
			if (!id) return []
			return [
				{
					id,
					tokens: numberFromProtocol(category?.tokens),
					shareBps: numberFromProtocol(category?.shareBps),
				},
			]
		}),
	}
}

/** Canonical `model/preferences` wire shape (ratified #12). */
type PreferencesOptionWire = {
	value: string
	label: string
	description?: string
	/** Present on `availableModels` entries: that model's effort choices. */
	availableEfforts?: PreferencesOptionWire[]
}

type ModelPreferencesWire = {
	model?: string
	reasoningEffort?: string
	availableModels?: PreferencesOptionWire[]
	availableEfforts?: PreferencesOptionWire[]
}

/** Canonical model preferences → the select options the config UI renders. */
function sessionConfigOptionsFromModelPreferences(preferences: ModelPreferencesWire): SessionConfigOption[] {
	const toSelectOptions = (entries?: PreferencesOptionWire[]) =>
		(entries ?? []).map((entry) => ({
			value: entry.value,
			name: entry.label,
			...(entry.description !== undefined ? { description: entry.description } : {}),
			...(entry.availableEfforts?.length
				? {
						availableEfforts: entry.availableEfforts.map((effort) => ({
							value: effort.value,
							name: effort.label,
							...(effort.description !== undefined ? { description: effort.description } : {}),
						})),
					}
				: {}),
		}))
	const options: SessionConfigOption[] = []
	if (preferences.model !== undefined || (preferences.availableModels?.length ?? 0) > 0) {
		options.push({
			type: "select",
			id: "model",
			name: "Model",
			description: "Controls the model used for this session",
			category: "model",
			currentValue: preferences.model ?? "",
			options: toSelectOptions(preferences.availableModels),
		} as SessionConfigOption)
	}
	if (preferences.reasoningEffort !== undefined || (preferences.availableEfforts?.length ?? 0) > 0) {
		options.push({
			type: "select",
			id: "thought_level",
			name: "Reasoning Effort",
			description: "Controls the model reasoning effort used for this session",
			category: "thought_level",
			currentValue: preferences.reasoningEffort ?? "",
			options: toSelectOptions(preferences.availableEfforts),
		} as SessionConfigOption)
	}
	return options
}

function parseTimestampMs(value: unknown): number | undefined {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value !== "string") return undefined
	const parsed = Date.parse(value)
	return Number.isFinite(parsed) ? parsed : undefined
}

function parseTitleState(value: unknown): string | undefined {
	if (typeof value === "string") return value
	if (value && typeof value === "object") {
		const keys = Object.keys(value as Record<string, unknown>)
		if (keys.length === 1) return keys[0]
	}
	return undefined
}

type LoadedSessionLimit = number | null
type SessionSettingsPatch = {
	modelID?: string
	reasoningEffort?: string
	mode?: string
	permissionProfile?: string
}

type SessionSettingsWaiter = {
	resolve: (session: Session | undefined) => void
	reject: (error: unknown) => void
}

type SessionSettingsQueue = {
	pending: SessionSettingsPatch | null
	waiters: SessionSettingsWaiter[]
	running: Promise<void> | null
	paused: boolean
}

const SESSION_SETTINGS_RETRY_DELAYS_MS = [250, 1_000, 2_000] as const
type PromptPartInput = {
	type: string
	text?: string
	url?: string
	filename?: string
	mime?: string
	mediaType?: string
}

function pathFromFileUri(uri: string): string | null {
	if (!uri.startsWith("file://")) return null
	try {
		const url = new URL(uri)
		let path = decodeURIComponent(url.pathname)
		if (/^\/[A-Za-z]:/.test(path)) path = path.slice(1)
		return path.replace(/\//g, "\\")
	} catch {
		return uri.slice("file://".length)
	}
}

export type PromptAsyncOutcome =
	| { outcome: "started"; turnId?: string }
	| { outcome: "queued"; queueItemId: string }

export type QueueWireEntry = {
	queueItemId: string
	position: number
	preview: string
	enqueuedAt?: string
	input?: Array<{ type: string; text?: string }>
}

function parseQueueWireEntries(value: unknown): QueueWireEntry[] {
	if (!Array.isArray(value)) return []
	return value
		.map((entry) => objectRecord(entry))
		.filter((entry): entry is Record<string, unknown> => !!entry)
		.map((entry) => ({
			queueItemId: String(entry.queueItemId ?? ""),
			position: Number(entry.position ?? 0),
			preview: String(entry.preview ?? ""),
			enqueuedAt: typeof entry.enqueuedAt === "string" ? entry.enqueuedAt : undefined,
			input: Array.isArray(entry.input)
				? entry.input.map((part) => {
						const record = objectRecord(part)
						return {
							type: String(record?.type ?? "text"),
							text: typeof record?.text === "string" ? record.text : undefined,
						}
					})
				: undefined,
		}))
		.filter((entry) => entry.queueItemId.length > 0)
		.sort((left, right) => left.position - right.position)
}

function userInputsFromPromptParts(parts: PromptPartInput[]): UserInput[] {
	const input: UserInput[] = []
	const text = parts
		.map((part) => (part.type === "text" ? (part.text ?? "") : ""))
		.join("\n")
		.trim()
	if (text || parts.every((part) => part.type !== "file")) {
		input.push({ type: "text", text })
	}
	for (const part of parts) {
		if (part.type !== "file" || !part.url) continue
		const path = pathFromFileUri(part.url)
		if (path) {
			input.push({
				type: "mention",
				uri: path,
			})
			continue
		}
		input.push({
			type: "text",
			text: `Resource ${part.filename ?? part.url}: ${part.url}`,
		})
	}
	return input
}

function normalizedHistoryLimit(limit: unknown): number | undefined {
	if (typeof limit !== "number" || !Number.isFinite(limit) || limit <= 0) return undefined
	return Math.floor(limit)
}

function loadedLimitCovers(loaded: LoadedSessionLimit | undefined, requested: number | undefined): boolean {
	if (loaded === undefined) return false
	if (loaded === null) return true
	return requested !== undefined && loaded >= requested
}

function mergeSessionSettingsPatch(
	base: SessionSettingsPatch | null,
	patch: SessionSettingsPatch,
): SessionSettingsPatch {
	return { ...(base ?? {}), ...patch }
}

function errorRecord(error: unknown): Record<string, unknown> | undefined {
	if (error && typeof error === "object") return error as Record<string, unknown>
	if (typeof error !== "string") return undefined
	try {
		return objectRecord(JSON.parse(error))
	} catch {
		return undefined
	}
}

/** Map a native turn failure onto the Desktop session/assistant error shape. */
function assistantErrorFromTurnFailure(
	turnStatus: string,
	turnError: Record<string, unknown> | undefined,
): { name: string; data: Record<string, unknown> } | undefined {
	const message =
		typeof turnError?.message === "string" && turnError.message.trim()
			? turnError.message.trim()
			: undefined
	if (!message) {
		// Follow-up `turn/completed` after TurnFailed has status failed but no
		// error payload — do not invent a generic message that would clobber UI.
		return undefined
	}
	const code =
		typeof turnError?.errorCode === "string"
			? turnError.errorCode
			: typeof turnError?.error_code === "string"
				? turnError.error_code
				: turnStatus === "failed"
					? "TurnFailed"
					: "Error"
	const details = objectRecord(turnError?.details)
	return {
		name: code,
		data: {
			message,
			...(code !== "Error" ? { code } : {}),
			...(details ?? {}),
		},
	}
}

function settingsErrorCode(error: unknown): string | undefined {
	const record = errorRecord(error)
	if (typeof record?.code === "string") return record.code
	if (error instanceof Error) {
		try {
			const messageRecord = objectRecord(JSON.parse(error.message))
			return typeof messageRecord?.code === "string" ? messageRecord.code : undefined
		} catch {
			return undefined
		}
	}
	return undefined
}

/** True when the Native server reports the session is gone / never existed. */
export function isSessionNotFoundError(error: unknown): boolean {
	const code = settingsErrorCode(error)
	const normalizedCode = code?.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
	if (normalizedCode === "session_not_found") return true
	const record = errorRecord(error)
	if (typeof record?.code === "string") {
		const recordCode = record.code.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
		if (recordCode === "session_not_found") return true
	}
	const message = error instanceof Error ? error.message : String(error)
	return (
		/session does not exist/i.test(message) ||
		/^session .+ not found$/i.test(message) ||
		/session id is not addressable by this server/i.test(message)
	)
}

function isTransientSessionSettingsError(error: unknown): boolean {
	const record = errorRecord(error)
	const code = settingsErrorCode(error)
	const normalizedCode = code?.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase()
	if (
		normalizedCode === "service_unavailable" ||
		normalizedCode === "temporary_unavailable" ||
		normalizedCode === "timeout"
	) {
		return true
	}
	const status = record?.status ?? record?.statusCode
	if (typeof status === "number" && status >= 500 && status <= 599) return true
	const message = error instanceof Error ? error.message : String(error)
	return /(?:^|\D)5\d{2}(?:\D|$)|timeout|timed out|network|fetch failed|connection|temporar(?:y|ily)|unavailable/i.test(message)
}

function waitForSessionSettingsRetry(delayMs: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, delayMs))
}

const initializePromises = new WeakMap<DevoNativeTransport, Promise<void>>()

export const DESKTOP_INITIALIZE_PARAMS = {
	protocolVersion: 1,
	_meta: { devo: { protocol: "native", typedItems: true } },
	clientCapabilities: {
		fs: { readTextFile: false, writeTextFile: false },
		terminal: false,
	},
	clientInfo: {
		name: "devo-desktop",
		title: "Devo Desktop",
		version: "0.1.0",
	},
} as const


class NativeClient {
	private transport: DevoNativeTransport | null = null
	private openPromise: Promise<void> | null = null
	private initialized = false
	private disposed = false
	private unsubscribeTransport: (() => void) | null = null
	private events = new AsyncEventQueue<GlobalEvent>()
	private sessions = new Map<string, Session>()
	private sessionDirectories = new Map<string, string>()
	private sessionStatuses = new Map<string, SessionStatus>()
	private promptStartedAtBySession = new Map<string, number>()
	/** Session transcript: Native ItemEnvelope keyed by itemId (no Message/Part dual). */
	private items = new Map<string, Map<string, NativeItemEnvelope>>()
	private loadedSessionLimits = new Map<string, LoadedSessionLimit>()
	private historyCursors = new Map<string, string | null>()
	private resumedSessions = new Set<string>()
	private sessionResumes = new Map<string, Promise<void>>()
	private subscriptionLoads = new Map<string, Promise<void>>()
	private directorySubscriptions = new Map<string, Promise<void>>()
	private sessionLists = new Map<string, Promise<Session[]>>()
	private configOptionsBySession = new Map<string, SessionConfigOption[]>()
	private configOptionsByDirectory = new Map<string, SessionConfigOption[]>()
	private pendingPermissions = new Map<string, PendingPermission>()
	private pendingQuestions = new Map<string, PendingQuestion>()
	private subscriptions = new Map<string, { subscriptionId: string; cursors: Array<{ streamId: string; seq: number }> }>()
	private subscriptionCursors = new Map<string, Array<{ streamId: string; seq: number }>>()
	private turnSessions = new Map<string, string>()
	private activeTurnIds = new Map<string, string>()
	private queueEntriesBySession = new Map<string, QueueWireEntry[]>()
	/** Native itemId → callId (for command output deltas). */
	private nativeItemCallIds = new Map<string, string>()
	private sessionDiscovery = new Map<string, Promise<Session | undefined>>()
	private sessionLoads = new Map<string, Promise<void>>()
	private sessionSettingsQueues = new Map<string, SessionSettingsQueue>()
	private lastEventTime = 0
	private referenceSearchSession: ReferenceSearchSession | null = null
	/** >0 while applying subscription create replay — must not bump sidebar sort keys. */
	private subscriptionReplayDepth = 0

	constructor(private readonly options: CreateDevoClientOptions) {}

	dispose(): void {
		if (this.disposed) return
		this.disposed = true
		const unsubscribe = this.unsubscribeTransport
		this.unsubscribeTransport = null
		try {
			unsubscribe?.()
		} finally {
			this.events.close()
		}
	}

	project = {
		list: async () => ({ data: await this.listProjects() }),
	}
	session = {
		list: async (params?: { limit?: number; roots?: boolean; search?: string }) => ({
			data: await this.listSessions(params),
		}),
		status: async () => ({ data: Object.fromEntries(this.sessionStatuses) }),
		create: async (_params?: { title?: string }) => ({ data: await this.createSession() }),
		promptAsync: async (params: {
			sessionId: string
			parts: PromptPartInput[]
			model?: unknown
			agent?: string
			variant?: string
			collaborationMode?: string
		}) => {
			const directory = this.sessionDirectories.get(params.sessionId) ?? this.options.directory ?? defaultCwd()
			const activityAt = Math.max(Date.now(), this.lastEventTime + 1)
			this.touchNativeSessionActivity(params.sessionId, activityAt)
			const wasBusy = this.sessionStatuses.get(params.sessionId)?.type === "busy"
			try {
				const result = await this.pushSessionQueue({
					sessionId: params.sessionId,
					parts: params.parts,
					collaborationMode: params.collaborationMode,
				})
				if (result.outcome === "started") {
					const promptStartedAt = Math.max(Date.now(), this.lastEventTime + 1)
					this.promptStartedAtBySession.set(params.sessionId, promptStartedAt)
					const busyStatus = { type: "busy" }
					this.sessionStatuses.set(params.sessionId, busyStatus)
					this.emit(directory, {
						type: "session.status",
						properties: { sessionId: params.sessionId, status: busyStatus },
					})
				}
				return { data: result }
			} catch (error) {
				if (!wasBusy && !this.activeTurnIds.has(params.sessionId)) {
					this.promptStartedAtBySession.delete(params.sessionId)
					this.completeOpenAssistantMessages(params.sessionId, directory, activityAt)
					const idleStatus = { type: "idle" }
					this.sessionStatuses.set(params.sessionId, idleStatus)
					this.emit(directory, {
						type: "session.status",
						properties: { sessionId: params.sessionId, status: idleStatus },
					})
				}
				this.emit(directory, sessionErrorEvent(params.sessionId, error))
				throw error
			}
		},
		queue: {
			list: async (params: { sessionId: string }) => {
				// Historical sessions show up in session/list but are not
				// addressable until session/resume. Composer refresh races
				// message load on open, so wait for load before queue/list.
				try {
					await this.ensureSessionReady(params.sessionId)
					const result = (await this.requestCanonical("session/queue/list", {
						sessionId: params.sessionId,
					})) as { entries?: unknown }
					const entries = parseQueueWireEntries(result.entries)
					this.emitQueueSnapshot(params.sessionId, entries, "sync")
					return { data: { entries } }
				} catch (error) {
					if (isSessionNotFoundError(error)) {
						this.emitQueueSnapshot(params.sessionId, [], "sync")
						return { data: { entries: [] } }
					}
					throw error
				}
			},
			push: async (params: {
				sessionId: string
				parts: PromptPartInput[]
				collaborationMode?: string
			}) => ({ data: await this.pushSessionQueue(params) }),
			update: async (params: {
				sessionId: string
				queueItemId: string
				parts?: PromptPartInput[]
				position?: number
			}) => {
				const result = (await this.requestCanonical("session/queue/update", {
					sessionId: params.sessionId,
					queueItemId: params.queueItemId,
					...(params.parts
						? { input: userInputsFromPromptParts(params.parts) }
						: {}),
					...(params.position !== undefined ? { position: params.position } : {}),
				})) as { entry?: unknown }
				const entry = objectRecord(result.entry)
				if (entry) {
					const entries = parseQueueWireEntries([entry])
					if (entries[0]) {
						const current = this.queueEntriesForSession(params.sessionId)
						const next = current.map((item) =>
							item.queueItemId === entries[0].queueItemId ? entries[0] : item,
						)
						if (!next.some((item) => item.queueItemId === entries[0].queueItemId)) {
							next.push(entries[0])
						}
						this.emitQueueSnapshot(
							params.sessionId,
							next.sort((left, right) => left.position - right.position),
							"updated",
						)
					}
				}
				return { data: result }
			},
			remove: async (params: { sessionId: string; queueItemId: string }) => {
				await this.requestCanonical("session/queue/remove", {
					sessionId: params.sessionId,
					queueItemId: params.queueItemId,
				})
				return { data: null }
			},
		},
		steer: async (params: { sessionId: string; parts: PromptPartInput[] }) => {
			const expectedTurnId = this.activeTurnIds.get(params.sessionId)
			if (!expectedTurnId) {
				throw new Error("No active turn to steer")
			}
			const result = await this.requestCanonical("turn/steer", {
				sessionId: params.sessionId,
				expectedTurnId,
				input: userInputsFromPromptParts(params.parts),
				idempotencyKey: crypto.randomUUID(),
			})
			return { data: result }
		},
		editMessage: async (params: { sessionId: string; itemId: string; text: string }) => {
			const directory = this.sessionDirectories.get(params.sessionId) ?? this.options.directory ?? defaultCwd()
			const promptStartedAt = Math.max(Date.now(), this.lastEventTime + 1)
			this.promptStartedAtBySession.set(params.sessionId, promptStartedAt)
			this.touchNativeSessionActivity(params.sessionId, promptStartedAt)
			try {
				const result = await this.requestCanonical("session/message/edit", {
					sessionId: params.sessionId,
					itemId: params.itemId,
					expectedRevision: 0,
					content: [{ type: "text", text: params.text }],
					idempotencyKey: crypto.randomUUID(),
				})
				if (this.sessionStatuses.get(params.sessionId)?.type !== "busy") {
					const busyStatus = { type: "busy" }
					this.sessionStatuses.set(params.sessionId, busyStatus)
					this.emit(directory, {
						type: "session.status",
						properties: { sessionId: params.sessionId, status: busyStatus },
					})
				}
				return { data: result }
			} catch (error) {
				this.promptStartedAtBySession.delete(params.sessionId)
				throw error
			}
		},
		abort: async (params: { sessionId: string }) => {
			const interruptParams: SessionInterruptParams = {
				scope: { scope: "session", sessionId: params.sessionId },
			}
			await this.request("session/interrupt", interruptParams)
		},
		/**
		 * Persists composer selections through one per-session queue. Durable
		 * metadata updates do not require a prior resume, so this path is safe
		 * while history is still loading and returns only after the server ACKs.
		 */
		updateSettings: async (params: SessionSettingsPatch & { sessionId: string }) => {
			return { data: await this.enqueueSessionSettings(params.sessionId, params) }
		},
		retrySettings: async (params: { sessionId: string }) => {
			return { data: await this.retrySessionSettings(params.sessionId) }
		},
		update: async (params: { sessionId: string; title: string }) => {
			// Canonical session/metadata/update (L2-DES-APP-008): the title
			// patch on the persist-first path; result is the canonical Session.
			const result = (await this.requestCanonical("session/metadata/update", {
				sessionId: params.sessionId,
				expectedVersion: 0,
				title: params.title,
			})) as { session?: Record<string, unknown> }
			const metadata = {
				...(result.session ?? {}),
				id: String(result.session?.id ?? params.sessionId),
				title: result.session?.title ?? params.title,
			}
			const session = this.rememberNativeSession(metadata)
			this.emit(session.directory ?? this.options.directory ?? defaultCwd(), {
				type: "session.updated",
				properties: { info: session, session },
			})
			return { data: session }
		},
		delete: async (params: { sessionId: string }) => {
			await this.requestCanonical("session/delete", { sessionId: params.sessionId })
			const { directory } = this.forgetSession(params.sessionId)
			this.emitSessionDeleted(params.sessionId, directory)
		},
		// `session.get` is a durable snapshot read. It intentionally does not
		// resume the server actor; callers that need history use `messages`,
		// while metadata updates can write the snapshot directly.
		get: async (params: { sessionId: string }) => ({
			data: await this.getSessionById(params.sessionId),
		}),
		diff: async (params: { sessionId: string }) => {
			try {
				const result = (await this.requestCanonical("workspace/changes/read", {
					sessionId: params.sessionId,
					scopes: ["uncommitted"],
					diffDetail: "full",
					maxDiffBytes: 2_000_000,
				})) as { views?: Array<Record<string, unknown>> }
				return {
					data: (result.views ?? [])
						.map((view) => view.unifiedDiff)
						.filter((diff): diff is string => typeof diff === "string" && diff.length > 0)
						.map((diff) => ({ diff })),
				}
			} catch (error) {
				if (isSessionNotFoundError(error)) {
					this.dropMissingSession(params.sessionId)
					return { data: [] }
				}
				throw error
			}
		},
		revert: async (params: { sessionId: string }) => ({
			data: this.sessions.get(params.sessionId),
		}),
		unrevert: async (params: { sessionId: string }) => ({
			data: this.sessions.get(params.sessionId),
		}),
		command: async (params: { sessionId: string; command: string; arguments?: string }) => {
			const suffix = params.arguments ? ` ${params.arguments}` : ""
			await this.session.promptAsync({
				sessionId: params.sessionId,
				parts: [{ type: "text", text: `/${params.command}${suffix}` }],
			})
		},
		summarize: async (params: { sessionId: string }) => {
			await this.ensureInitialized()
			await this.ensureSessionSubscription(params.sessionId)
			await this.requestCanonical("session/compact/start", {
				sessionId: params.sessionId,
			})
		},
		messages: async (params: { sessionId: string; limit?: number }) => ({
			data: await this.sessionMessages(params.sessionId, normalizedHistoryLimit(params.limit)),
		}),
		fork: async (params: {
			sessionId: string
			atTurnId?: string
			cut?: "through" | "before"
		}) => {
			await this.ensureInitialized()
			const result = (await this.requestCanonical("session/fork", {
				sessionId: params.sessionId,
				...(params.atTurnId ? { atTurnId: params.atTurnId } : {}),
				...(params.cut ? { cut: params.cut } : {}),
			})) as { session?: Record<string, unknown> }
			const sessionValue = result.session
			if (!sessionValue) {
				throw new Error("session/fork returned no session")
			}
			const session = this.rememberNativeSession(sessionValue)
			await this.ensureSessionSubscription(session.id)
			await this.loadSession(session.id)
			this.emit(session.directory ?? this.options.directory ?? defaultCwd(), {
				type: "session.created",
				properties: { info: session, session },
			})
			return { data: session }
		},
	}

	turn = {
		start: async (params: {
			sessionId: string
			parts: PromptPartInput[]
			model?: unknown
			variant?: string
			cwd?: string | null
			collaborationMode?: string
		}) => {
			// Model/variant selections are persisted by the composer's
			// persist-on-selection path (session.updateSettings) and must NOT
			// be re-derived here: callers used to pass fallback-resolved
			// models (request slugs, defaults) which then overwrote the
			// user's persisted choices on every send. Only the collaboration
			// mode rides along — canonical turn/start carries no mode, and
			// toggling mode without sending must still apply to the next turn.
			if (params.collaborationMode) {
				const settingsPatch: SessionSettingsPatch = {
					mode: params.collaborationMode,
				}
				await this.enqueueSessionSettings(params.sessionId, settingsPatch)
			}
			await this.ensureSessionSubscription(params.sessionId)
			const result = (await this.requestCanonical("turn/start", {
				sessionId: params.sessionId,
				input: userInputsFromPromptParts(params.parts),
				idempotencyKey: crypto.randomUUID(),
			})) as { turn: unknown }
			return { data: result }
		},
	}

	task = {
		startAgent: async (params: {
			sessionId: string
			prompt: string
			forkTurns?: string
			maxTurns?: number
			toolPolicy?: "inherit" | "deny_all"
			ephemeral?: boolean
		}) => {
			await this.ensureInitialized()
			const result = (await this.requestCanonical("task/start", {
				kind: "agent",
				sessionId: params.sessionId,
				input: [{ type: "text", text: params.prompt }],
				...(params.forkTurns ? { forkTurns: params.forkTurns } : {}),
				...(params.maxTurns !== undefined ? { maxTurns: params.maxTurns } : {}),
				...(params.toolPolicy ? { toolPolicy: params.toolPolicy } : {}),
				ephemeral: params.ephemeral ?? false,
				idempotencyKey: crypto.randomUUID(),
			})) as { itemId?: string; item_id?: string }
			return {
				data: {
					itemId: String(result.itemId ?? result.item_id ?? ""),
				},
			}
		},
	}

	question = {
		reply: async (params: { requestId: string; answers: QuestionAnswer[] }) => {
			await this.respondToQuestion(params.requestId, params.answers, "question.replied")
		},
		reject: async (params: { requestId: string }) => {
			await this.respondToQuestion(params.requestId, [], "question.rejected")
		},
	}

	permission = {
		respond: async (params: {
			sessionId: string
			permissionId: string
			response: PermissionResponse
		}) => {
			await this.respondToPermission(params.permissionId, params.response)
		},
		reply: async (params: { requestId: string; reply?: PermissionResponse }) => {
			await this.respondToPermission(params.requestId, params.reply ?? "reject")
		},
	}

	instance = {
		dispose: async () => {},
	}

	global = {
		dispose: async () => {},
		event: async () => {
			await this.ensureKnownSessionSubscriptions()
			return { stream: this.events }
		},
		config: {
			update: async (_params: unknown) => ({ data: null }),
		},
	}

	event = {
		subscribe: async () => {
			await this.ensureKnownSessionSubscriptions()
			return { stream: this.events }
		},
	}

	workspace = {
		changes: {
			read: async (params: WorkspaceChangesReadOptions) => {
				// Pass Native WorkspaceChangesReadParams through; only default diffDetail.
				const wireParams: Record<string, unknown> = {
					sessionId: params.sessionId,
					scopes: params.scopes,
					diffDetail: params.diffDetail ?? "summary",
				}
				if (params.cwd !== undefined) wireParams.cwd = params.cwd
				if (params.baseBranch !== undefined) wireParams.baseBranch = params.baseBranch
				if (params.turnId !== undefined) wireParams.turnId = params.turnId
				if (params.maxDiffBytes !== undefined) {
					wireParams.maxDiffBytes = Number(params.maxDiffBytes)
				}
				if (params.ignoreWhitespace !== undefined) {
					wireParams.ignoreWhitespace = params.ignoreWhitespace
				}
				if (params.paths !== undefined) {
					wireParams.paths = params.paths
				}
				if (params.includeFileSides !== undefined) {
					wireParams.includeFileSides = params.includeFileSides
				}
				const data = (await this.requestCanonical(
					"workspace/changes/read",
					wireParams,
				).catch((error) => {
					if (isSessionNotFoundError(error)) {
						this.dropMissingSession(params.sessionId)
						return { views: [] }
					}
					throw error
				})) as WorkspaceChangesReadResult
				return { data }
			},
		},
	}

	command = {
		list: async () => ({ data: [{ name: "compact", description: "Compact the session" }] }),
	}

	// User requirement: Desktop's composer status area needs direct goal state
	// controls, while the existing /goal trigger remains available for entry.
	private async canonicalGoalTransition(method: string, sessionId: string): Promise<unknown> {
		const current = (await this.requestCanonical("session/goal/read", {
			sessionId,
		})) as { goal?: { id?: string } | null }
		const expectedGoalId = current.goal?.id
		if (!expectedGoalId) throw new Error("session has no active goal")
		return this.requestCanonical(method, {
			sessionId,
			expectedGoalId,
		})
	}

	goal = {
		status: async (params: { sessionId: string }) => {
			try {
				const result = (await this.requestCanonical("session/goal/read", {
					sessionId: params.sessionId,
				})) as { goal?: unknown }
				return { data: result.goal }
			} catch (error) {
				if (isSessionNotFoundError(error)) {
					this.dropMissingSession(params.sessionId)
					return { data: null }
				}
				throw error
			}
		},
		pause: async (params: { sessionId: string }) => {
			const result = (await this.canonicalGoalTransition(
				"session/goal/pause",
				params.sessionId,
			)) as { goal?: unknown }
			return { data: result.goal }
		},
		resume: async (params: { sessionId: string }) => {
			const result = (await this.canonicalGoalTransition(
				"session/goal/resume",
				params.sessionId,
			)) as { goal?: unknown }
			return { data: result.goal }
		},
		clear: async (params: { sessionId: string }) => {
			const result = await this.canonicalGoalTransition(
				"session/goal/clear",
				params.sessionId,
			)
			return { data: result }
		},
	}

	find = {
		// @ mention file search uses connection-local search/* RPC + notifications.
		files: async (params: { query: string }) => {
			const session = this.ensureReferenceSearchSession()
			await session.startOrUpdate(params.query)
			return { data: session.filePaths() }
		},
	}

	referenceSearch = {
		startOrUpdate: async (params: { query: string }) => ({
			data: await this.ensureReferenceSearchSession().startOrUpdate(params.query),
		}),
		cancel: async () => {
			await this.ensureReferenceSearchSession().cancel()
			return { data: null }
		},
		subscribe: (listener: (snapshot: ReferenceSearchSnapshot) => void) =>
			this.ensureReferenceSearchSession().subscribe(listener),
		getState: () => this.ensureReferenceSearchSession().getState(),
	}

	worktree = {
		list: async () => {
			const result = await this.requestCanonical("workspace/worktree/list", { cwd: this.options.directory ?? defaultCwd() }) as { worktrees: Array<{ directory: string }> }
			return { data: result.worktrees.map((worktree) => worktree.directory) }
		},
		create: async (params: { worktreeCreateInput: { name: string; startCommand?: string } }) => ({
			data: await this.requestCanonical("workspace/worktree/create", { cwd: this.options.directory ?? defaultCwd(), name: params.worktreeCreateInput.name }),
		}),
		remove: async (params: { worktreeRemoveInput: { directory: string } }) => ({
			data: await this.requestCanonical("workspace/worktree/remove", { cwd: this.options.directory ?? defaultCwd(), directory: params.worktreeRemoveInput.directory }),
		}),
		reset: async (params: { worktreeResetInput: { directory: string } }) => ({
			data: await this.requestCanonical("workspace/worktree/reset", { cwd: this.options.directory ?? defaultCwd(), directory: params.worktreeResetInput.directory }),
		}),
	}

	config = {
		providers: async () => ({
			data: providerDataFromConfigOptions(await this.ensureCurrentConfigOptions()),
		}),
		get: async () => ({ data: configDataFromConfigOptions(await this.ensureCurrentConfigOptions()) }),
		setOption: async (params: { configID: string; value: string }) => ({
			data: configDataFromConfigOptions(
				await this.setDefaultConfigOption(params.configID, params.value),
			),
		}),
	}

	vcs = {
		get: async () => ({ data: null }),
	}

	app = {
		agents: async () => ({ data: [] }),
		skills: async () => {
			const result = (await this.requestCanonical("skill/list", {
				...(this.options.directory ? { cwd: this.options.directory } : {}),
				forceReload: false,
			})) as { skills?: unknown[] }
			return { data: result.skills ?? [] }
		},
		setSkillEnabled: async (params: { path: string; enabled: boolean }) => {
			const result = (await this.requestCanonical("skill/set_enabled", {
				path: params.path,
				enabled: params.enabled,
				...(this.options.directory ? { cwd: this.options.directory } : {}),
			})) as { skills?: unknown[] }
			return { data: result.skills ?? [] }
		},
	}

	context = {
		usage: {
			read: async (params: { sessionId: string }) => {
				const result = (await this.requestCanonical("context/usage/read", {
					sessionId: params.sessionId,
				})) as { occupancy?: unknown }
				this.emitContextUsage(params.sessionId, result.occupancy)
				return { data: result.occupancy }
			},
		},
	}

	mcp = {
		list: async () => {
			const result = (await this.requestCanonical("mcp/list", {})) as { servers?: unknown[] }
			return { data: result.servers ?? [] }
		},
		tools: async (params: { name: string }) => {
			const result = (await this.requestCanonical("mcp/tools", { name: params.name })) as {
				tools?: unknown[]
			}
			return { data: result.tools ?? [] }
		},
		setEnabled: async (params: { name: string; enabled: boolean }) => ({
			data: await this.requestCanonical("mcp/set_enabled", params),
		}),
	}

	private modelPreferencesWrite: Promise<unknown> = Promise.resolve()

	model = {
		preferences: {
			write: (params: { patch: { model?: string; reasoningEffort?: string } }) => {
				const write = this.modelPreferencesWrite.catch(() => undefined).then(async () => {
					const result = await this.requestCanonical("model/preferences/write", {
						...params, ...(this.options.directory ? { cwd: this.options.directory } : {}),
					}) as { preferences: ModelPreferencesWire }
					this.rememberDirectoryConfigOptions(this.options.directory ?? defaultCwd(),
						sessionConfigOptionsFromModelPreferences(result.preferences))
					return { data: result }
				})
				this.modelPreferencesWrite = write
				return write
			},
		},
		list: async (): Promise<{ data: ModelListResult }> => ({
			data: await this.requestCanonical("model/list", {}) as ModelListResult,
		}),
		refreshCatalog: async (params: ModelCatalogRefreshParams): Promise<{ data: ModelCatalogRefreshResult }> => {
			const data = await this.requestCanonical("model/catalog/refresh", params) as ModelCatalogRefreshResult
			if (data.status === "updated") this.invalidateConfigOptionCaches()
			return { data }
		},
	}

	credential = {
		/** Native returns credential metadata only, never secret values. */
		list: async (): Promise<{ data: CredentialListResult }> => ({
			data: await this.requestCanonical("credential/list", {}) as CredentialListResult,
		}),
	}

	provider = {
		list: async () => {
			const data = (await this.requestCanonical("provider/list", {})) as ProviderListResult
			return { data }
		},
		validate: async (params: ProviderValidateParams) => {
			const data = (await this.requestCanonical("provider/validate", params)) as ProviderValidateResult
			return { data }
		},
		upsert: async (params: ProviderUpsertParams) => {
			const data = (await this.requestCanonical("provider/upsert", params)) as ProviderUpsertResult
			this.invalidateConfigOptionCaches()
			return { data }
		},
		disconnect: async (params: ProviderDisconnectParams): Promise<ProviderDisconnectResult> => {
			const result = (await this.requestCanonical("provider/disconnect", params)) as ProviderDisconnectResult
			this.invalidateConfigOptionCaches()
			return result
		},
		modelRemove: async (params: ProviderModelRemoveParams): Promise<ProviderModelRemoveResult> => {
			const result = (await this.requestCanonical("provider/model/remove", params)) as ProviderModelRemoveResult
			this.invalidateConfigOptionCaches()
			return result
		},
		discover: async (params: ProviderDiscoverParams): Promise<ProviderDiscoverResult> => {
			const result = (await this.requestCanonical("provider/discover", params)) as ProviderDiscoverResult
			// Discover mutates the connection model directory; composer selectors
			// read model/preferences which must not keep a pre-discover snapshot.
			this.invalidateConfigOptionCaches()
			return result
		},
		auth: async () => ({ data: [] }),
		oauth: {
			authorize: async (_params: unknown) => ({ data: null }),
			callback: async (_params: unknown) => ({ data: null }),
		},
	}
	private async listProjects(): Promise<Project[]> {
		const sessions = await this.listSessions()
		const byDirectory = new Map<string, Project>()
		for (const session of sessions) {
			const directory = session.directory ?? this.options.directory
			if (!directory) continue
			const previous = byDirectory.get(directory)
			const updated = session.time.lastActivity ?? session.time.updated ?? session.time.created
			if (previous) {
				previous.time.updated = Math.max(previous.time.updated ?? 0, updated)
				continue
			}
			byDirectory.set(directory, {
				id: stableId(directory),
				name: directory.split(/[\\/]/).filter(Boolean).at(-1) ?? directory,
				worktree: directory,
				path: { root: directory },
				time: { created: session.time.created, updated },
				sandboxes: [],
			})
		}
		if (byDirectory.size === 0 && this.options.directory) {
			byDirectory.set(this.options.directory, {
				id: stableId(this.options.directory),
				name: this.options.directory.split(/[\\/]/).filter(Boolean).at(-1) ?? this.options.directory,
				worktree: this.options.directory,
				path: { root: this.options.directory },
				time: { created: Date.now(), updated: Date.now() },
				sandboxes: [],
			})
		}
		return [...byDirectory.values()]
	}

	private async listSessions(params?: { limit?: number; roots?: boolean; search?: string }): Promise<Session[]> {
		const key = JSON.stringify(params ?? {})
		const pending = this.sessionLists.get(key)
		if (pending) return pending
		const load = this.listSessionsOnce(params)
		this.sessionLists.set(key, load)
		try { return await load } finally {
			if (this.sessionLists.get(key) === load) this.sessionLists.delete(key)
		}
	}

	private async listSessionsOnce(params?: { limit?: number; roots?: boolean; search?: string }): Promise<Session[]> {
		await this.ensureInitialized()
		const sessions: Session[] = []
		let cursor: string | undefined
		do {
			const result = (await this.requestCanonical("session/list", {
				cwds: this.options.directory ? [this.options.directory] : [],
				...(params?.search ? { search: params.search } : {}),
				...(cursor ? { cursor } : {}),
				...(params?.limit ? { limit: params.limit } : {}),
			})) as { data?: Array<Record<string, unknown>>; nextCursor?: string | null }
			sessions.push(...(result.data ?? []).map((info) => this.rememberNativeSession(info)))
			cursor = result.nextCursor ?? undefined
			if (params?.limit && !params.search && sessions.length >= params.limit) break
			if (params?.limit && params.search) {
				const matching = sessions.filter((session) =>
					(session.title ?? session.id).toLowerCase().includes(params.search!.toLowerCase()),
				)
				if (matching.length >= params.limit) break
			}
		} while (cursor)
		const filtered = params?.search
			? sessions.filter((session) =>
					(session.title ?? session.id).toLowerCase().includes(params.search!.toLowerCase()),
				)
			: sessions
		return filtered.slice(0, params?.limit ?? filtered.length)
	}

	private async createSession(): Promise<Session> {
		await this.ensureInitialized()
		const cwd = this.options.directory ?? defaultCwd()
		const result = (await this.requestCanonical("session/new", {
			cwd,
			idempotencyKey: crypto.randomUUID(),
		})) as { session: Record<string, unknown> }
		const session = this.rememberNativeSession(result.session)
		await this.ensureSessionSubscription(session.id)
		this.resumedSessions.add(session.id)
		this.emit(session.directory ?? cwd, {
			type: "session.created",
			properties: { info: session, session },
		})
		return session
		}
	private async sessionMessages(
		sessionId: string,
		limit?: number,
	): Promise<Array<{ info: NativeItemEnvelope }>> {
		try {
			await this.loadSession(sessionId, limit)
		} catch (error) {
			if (isSessionNotFoundError(error)) return []
			throw error
		}
		const items = [...(this.items.get(sessionId)?.values() ?? [])]
		return recentNativeItems(items, limit).map((info) => ({ info }))
	}
	private async loadSession(sessionId: string, limit?: number): Promise<void> {
		// A concurrent queue/read or wider history request must not claim that a
		// narrower in-flight window covers it. Recheck coverage after joining.
		while (!loadedLimitCovers(this.loadedSessionLimits.get(sessionId), limit)) {
			const pending = this.sessionLoads.get(sessionId)
			if (pending) { await pending; continue }
			const load = this.loadSessionOnce(sessionId, limit)
			this.sessionLoads.set(sessionId, load)
			try { await load } catch (error) {
				if (isSessionNotFoundError(error) && (this.sessions.has(sessionId) || this.items.has(sessionId))) {
					this.dropMissingSession(sessionId)
				}
				throw error
			} finally {
				if (this.sessionLoads.get(sessionId) === load) this.sessionLoads.delete(sessionId)
			}
		}
	}

	private dropMissingSession(sessionId: string): void {
		const { directory } = this.forgetSession(sessionId)
		this.subscriptions.delete(sessionId)
		this.sessionSettingsQueues.delete(sessionId)
		this.emitSessionDeleted(sessionId, directory)
	}

	private async ensureSessionReady(sessionId: string): Promise<void> {
		if (this.resumedSessions.has(sessionId)) return
		const pending = this.sessionResumes.get(sessionId)
		if (pending) return pending
		const resume = this.resumeSessionOnce(sessionId)
		this.sessionResumes.set(sessionId, resume)
		try { await resume } finally {
			if (this.sessionResumes.get(sessionId) === resume) this.sessionResumes.delete(sessionId)
		}
	}

	private async resumeSessionOnce(sessionId: string): Promise<void> {
		try {
			const resumed = (await this.requestCanonical("session/resume", { sessionId })) as {
				session: Record<string, unknown>; lastContextOccupancy?: unknown; last_context_occupancy?: unknown
			}
			const enriched = this.rememberNativeSession(resumed.session)
			this.emit(enriched.directory ?? this.options.directory ?? defaultCwd(), {
				type: "session.updated", properties: { info: enriched, session: enriched },
			})
			this.emitContextUsage(sessionId, resumed.lastContextOccupancy ?? resumed.last_context_occupancy)
			await this.ensureSessionSubscription(sessionId)
			this.resumedSessions.add(sessionId)
			this.sessionsNeedingResume.delete(sessionId)
		} catch (error) {
			if (isSessionNotFoundError(error)) this.dropMissingSession(sessionId)
			throw error
		}
	}

	private async loadSessionOnce(sessionId: string, limit?: number): Promise<void> {
		await this.ensureSessionReady(sessionId)
		let loaded = this.loadedSessionLimits.get(sessionId) ?? 0
		let cursor: string | null = this.historyCursors.get(sessionId) ?? "tail"
		do {
			const page = (await this.requestCanonical("session/items/list", {
				sessionId, cursor, limit: Math.min(200, limit === undefined ? 200 : Math.max(1, limit - loaded)),
			})) as { data?: Array<Record<string, unknown>>; nextCursor?: string | null }
			for (const item of page.data ?? []) this.handleNativeItemEnvelope(item, nativeItemNotificationMethod(item))
			loaded += (page.data ?? []).length
			cursor = page.nextCursor ?? null
			this.historyCursors.set(sessionId, cursor)
			this.loadedSessionLimits.set(sessionId, cursor ? loaded : null)
		} while (cursor && (limit === undefined || loaded < limit))
	}

	private async getSessionById(sessionId: string): Promise<Session | undefined> {
		const session = this.sessions.get(sessionId)
		if (session) return session
		return this.discoverSession(sessionId)
	}

	private async discoverSession(sessionId: string): Promise<Session | undefined> {
		const pending = this.sessionDiscovery.get(sessionId)
		if (pending) return pending
		const discovery = this.listSessions()
			.then((sessions) => sessions.find((session) => session.id === sessionId))
			.finally(() => {
				this.sessionDiscovery.delete(sessionId)
			})
		this.sessionDiscovery.set(sessionId, discovery)
		return discovery
	}

	private rememberNativeSession(info: Record<string, unknown>): Session {
		const id = String(info.id ?? "")
		if (!id) throw new Error("Native session is missing id")
		const existing = this.sessions.get(id)
		const usage = objectRecord(info.usage)
		const total = objectRecord(usage?.total)
		const created = parseTimestampMs(info.createdAt) ?? existing?.time.created ?? Date.now()
		const wireActivity = parseTimestampMs(info.lastActivityAt)
		const existingActivity = Math.max(existing?.time.lastActivity ?? 0, existing?.time.updated ?? 0)
		// Never let resume/snapshot lower a known activity timestamp — that alone
		// can reshuffle the sidebar. Prefer the newer of wire vs in-memory.
		const updated =
			wireActivity != null
				? Math.max(wireActivity, existingActivity)
				: existingActivity > 0
					? existingActivity
					: created
		const parent = objectRecord(info.parent)
		const forkFromId =
			typeof info.forkFromId === "string"
				? info.forkFromId
				: typeof info.fork_from_id === "string"
					? info.fork_from_id
					: existing?.forkFromId
		const atTurnId =
			typeof info.atTurnId === "string"
				? info.atTurnId
				: typeof info.fork_at_turn_id === "string"
					? info.fork_at_turn_id
					: existing?.atTurnId
		const titleState = parseTitleState(info.titleState ?? info.title_state)
		// Persisted per-session turn settings (model / reasoning effort / mode).
		// The server restores these on resume; without them the Desktop composer
		// cannot re-seed per-session selections after a restart and every
		// session falls back to the project default.
		const wireModel = objectRecord(info.model)
		const wireSettings = objectRecord(info.settings)
		const sessionModel = wireModel
			? {
					provider:
						typeof wireModel.provider === "string"
							? wireModel.provider
							: existing?.model?.provider,
					model:
						typeof wireModel.model === "string"
							? wireModel.model
							: existing?.model?.model,
					reasoningEffort:
						stringOrUndefined(wireModel.reasoningEffort ?? wireModel.reasoning_effort) ??
						existing?.model?.reasoningEffort,
			  }
			: existing?.model
		const sessionSettings = wireSettings
			? {
					mode: stringOrUndefined(wireSettings.mode) ?? existing?.settings?.mode,
					reasoningEffort:
						stringOrUndefined(wireSettings.reasoningEffort ?? wireSettings.reasoning_effort) ??
						existing?.settings?.reasoningEffort,
					permissionProfile:
						stringOrUndefined(wireSettings.permissionProfile ?? wireSettings.permission_profile) ??
						existing?.settings?.permissionProfile,
			  }
			: existing?.settings
		const session: Session = {
			id,
			title: typeof info.title === "string" ? info.title : existing?.title,
			titleState: titleState ?? existing?.titleState ?? "Unset",
			parentId: typeof parent?.sessionId === "string" ? parent.sessionId : existing?.parentId,
			forkFromId,
			atTurnId,
			time: { created, updated, lastActivity: updated },
			directory: String(info.cwd ?? existing?.directory ?? this.options.directory ?? defaultCwd()),
			model: sessionModel,
			settings: sessionSettings,
			totalInputTokens: Number(total?.inputTokens ?? existing?.totalInputTokens ?? 0),
			totalOutputTokens: Number(total?.outputTokens ?? existing?.totalOutputTokens ?? 0),
			totalTokens: Number(total?.totalTokens ?? existing?.totalTokens ?? 0),
			totalCacheCreationTokens: Number(total?.cacheCreationInputTokens ?? existing?.totalCacheCreationTokens ?? 0),
			totalCacheReadTokens: Number(total?.cacheReadInputTokens ?? existing?.totalCacheReadTokens ?? 0),
			promptTokenEstimate: Number(total?.inputTokens ?? existing?.promptTokenEstimate ?? 0),
			lastQueryTotalTokens: existing?.lastQueryTotalTokens ?? 0,
		}
		this.sessions.set(id, session)
		this.sessionDirectories.set(id, session.directory ?? defaultCwd())
		// Durable snapshots (session/list, resume, metadata) often report Idle
		// even while a turn is live; live busy/idle rides turn/* and
		// session/statusChanged. Never downgrade a known in-flight status from
		// a snapshot — otherwise delete-refill list calls clear "working" UI.
		const snapshotBusy = String(info.status).toLowerCase() === "active"
		const existingStatus = this.sessionStatuses.get(id)
		const existingInFlight =
			existingStatus?.type === "busy" || existingStatus?.type === "retry"
		this.sessionStatuses.set(
			id,
			snapshotBusy ? { type: "busy" } : existingInFlight ? existingStatus : { type: "idle" },
		)
		return session
	}

	/**
	 * Advances session lastActivity and emits session.updated so Desktop atoms
	 * (agentsAtom / sidebar) recompute without a session/list refresh.
	 * Native turn/item traffic does not carry session/metadataUpdated for
	 * activity-only bumps — only title updates do — so the client owns live sync.
	 *
	 * Skipped during subscription replay: historical turn/item envelopes must
	 * not reshuffle the sidebar when the user merely opens a session.
	 */
	private touchNativeSessionActivity(sessionId: string, at = Date.now()): void {
		if (this.subscriptionReplayDepth > 0) return
		const session = this.sessions.get(sessionId)
		if (!session) return
		const previous = Math.max(session.time.lastActivity ?? 0, session.time.updated ?? 0)
		if (at <= previous) return
		session.time.lastActivity = at
		session.time.updated = at
		const directory =
			this.sessionDirectories.get(sessionId) ?? session.directory ?? this.options.directory ?? defaultCwd()
		this.emit(directory, {
			type: "session.updated",
			properties: { info: session, session },
		})
	}

	private async ensureInitialized(): Promise<void> {
		if (this.initialized) return
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		if (this.initialized) return

		const transport = this.transport
		let promise = initializePromises.get(transport)
		if (!promise) {
			promise = this.request("initialize", DESKTOP_INITIALIZE_PARAMS).then(() => {})
			initializePromises.set(transport, promise)
		}
		try {
			await promise
		} catch (error) {
			// A failed handshake must not block retries on a still-open transport.
			// Another caller may have already replaced this entry with a new attempt.
			if (initializePromises.get(transport) === promise) initializePromises.delete(transport)
			throw error
		}
		this.initialized = true
	}

	private async open(): Promise<void> {
		if (this.disposed) throw new Error("Devo Native client has been disposed")
		if (this.transport) return
		if (this.openPromise) return this.openPromise
		this.openPromise = Promise.resolve()
			.then(() => {
				if (this.disposed) throw new Error("Devo Native client has been disposed")
				this.transport = this.options.transport ?? createIpcTransport()
				this.unsubscribeTransport = this.transport.subscribe((event) => this.handleTransportEvent(event))
			})
			.finally(() => {
				this.openPromise = null
			})
		return this.openPromise
	}

	private async request(method: string, params: unknown): Promise<unknown> {
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		const validParams = assertValidProtocolPayload({
			method,
			direction: "outgoingRequest",
			payload: params,
		})
		const result = await this.transport.request(method, validParams, this.options.directory)
		if (method === "subscription/create" || method === "subscription/update") {
			return this.validateSubscriptionResult(method, result)
		}
		return assertValidProtocolPayload({
			method,
			direction: "incomingResult",
			payload: result,
		})
	}

	/**
	 * Subscription results carry persisted event replay. A server build whose
	 * event generation differs from this client's schema can include a
	 * notification method this bundle does not recognize; replay processing
	 * ignores unknown methods anyway, so drop those envelopes instead of
	 * failing the whole event stream. Dropped envelopes are logged
	 * (rate-limited) so the generation skew stays observable.
	 */
	private validateSubscriptionResult(method: string, result: unknown): unknown {
		const { payload, dropped } = dropUnknownReplayEnvelopes(result)
		if (dropped.length > 0) reportDroppedReplayEnvelopes(method, dropped)
		return assertValidProtocolPayload({
			method,
			direction: "incomingResult",
			payload,
		})
	}

	/** Native RPC path shared by all first-party Desktop consumers. */
	private async requestCanonical(method: string, params: unknown): Promise<unknown> {
		await this.ensureInitialized()
		return this.request(method, params)
	}

	private ensureReferenceSearchSession(): ReferenceSearchSession {
		if (!this.referenceSearchSession) {
			const cwd = this.options.directory ?? defaultCwd()
			this.referenceSearchSession = new ReferenceSearchSession(
				(method, params) => this.requestCanonical(method, params),
				cwd,
			)
		}
		return this.referenceSearchSession
	}

    private recoveryListeners = new Set<() => void>()

    readonly turnRecovery = {
        read: async (sessionId: string) => this.request("turn/recovery/read", { sessionId }),
        resume: async (sessionId: string, recovery: { turnId: string; revision: number }, idempotencyKey: string) => {
            const result = await this.request("turn/resume", {
                sessionId, expectedTurnId: recovery.turnId, recoveryRevision: recovery.revision, idempotencyKey,
            })
            return result
        },
        cancel: async (sessionId: string) => this.session.abort({ sessionId }),
        subscribe: (listener: () => void) => {
            this.recoveryListeners.add(listener)
            return () => { this.recoveryListeners.delete(listener) }
        },
    }

	private handleTransportEvent(event: DevoNativeTransportEvent): void {
		if (this.disposed) return
        if (event.type === "closed" || (event.type === "notification" &&
            (event.method?.startsWith("turn/") || event.method === "session/metadataUpdated"))) {
            for (const listener of this.recoveryListeners) listener()
        }

		if (event.type === "closed") {
			if (this.transport) {
				initializePromises.delete(this.transport)
			}
			this.initialized = false
			this.events.close()
			this.events = new AsyncEventQueue<GlobalEvent>()
			this.pendingPermissions.clear()
			this.pendingQuestions.clear()
			this.subscriptions.clear()
			for (const sessionId of this.sessions.keys()) this.sessionsNeedingResume.add(sessionId)
			this.loadedSessionLimits.clear()
			this.historyCursors.clear()
			this.resumedSessions.clear()
			this.directorySubscriptions.clear()
			this.turnSessions.clear()
			this.nativeItemCallIds.clear()
			this.referenceSearchSession = null
			return
		}
		if (event.type === "notification" && event.method && event.params) {
			if (this.handleNativeNotification(event.method, event.params)) return
		}
		if (
			event.type === "notification" &&
			event.method &&
			event.params &&
			this.referenceSearchSession?.handleNotification(event.method, event.params)
		) {
			return
		}
		if (event.type === "request" && event.id !== undefined && event.method) {
			this.handleNativeServerRequest(event.id, event.method, event.params)
		}
	}

	private validateTransportPayload<T>(
		method: string,
		direction:
			| "incomingNotification"
			| "incomingRequest",
		payload: unknown,
	): T | null {
		try {
			return assertValidProtocolPayload<T>({ method, direction, payload })
		} catch (error) {
			this.emitProtocolValidationError(method, payload, error)
			return null
		}
	}

	private handleNativeServerRequest(id: JsonRpcId, method: string, params: unknown): boolean {
		if (
			method === "approval/command/request" ||
			method === "approval/fileChange/request" ||
			method === "approval/permission/request" ||
			method === "session/goal/completionApproval/request"
		) {
			const value = objectRecord(
				this.validateTransportPayload(method, "incomingRequest", params),
			) ?? {}
			const approvalId = String(value.approvalId ?? value.requestId ?? "")
			if (!approvalId) return true
			const availableScopes = knownApprovalScopes(
				Array.isArray(value.availableScopes) ? value.availableScopes.map(String) : undefined,
			)
			const existing = this.pendingPermissions.get(approvalId)
			this.pendingPermissions.set(approvalId, {
				id,
				method,
				sessionId: existing?.sessionId,
				options: [],
				availableScopes,
				native: true,
			})
			const sessionId = existing?.sessionId
			if (sessionId) {
				const directory =
					this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
				this.emitPermissionAsked(sessionId, directory, approvalId, value, availableScopes)
			}
			return true
		}
		if (method === "userInput/request") {
			const value = objectRecord(
				this.validateTransportPayload(method, "incomingRequest", params),
			) ?? {}
			const requestId = String(value.requestId ?? "")
			if (!requestId) return true
			const existing = this.pendingQuestions.get(requestId)
			const questions = (Array.isArray(value.questions) ? value.questions : []).map(questionInfoFromNative)
			this.pendingQuestions.set(requestId, {
				id,
				method,
				sessionId: existing?.sessionId ?? "",
				questions: existing?.questions.length ? existing.questions : questions,
			})
			return true
		}
		return false
	}

	private handleNativeNotification(method: string, params: unknown): boolean {
		const value = objectRecord(params) ?? {}
		if (method === "provider/authStale") {
			const providerId = String(value.providerId ?? value.provider_id ?? "")
			const reason =
				typeof value.reason === "string" && value.reason.trim().length > 0
					? value.reason
					: undefined
			this.emit(this.options.directory ?? defaultCwd(), {
				type: "provider.authStale",
				properties: {
					providerId,
					reason,
				},
			})
			return true
		}
		if (method === "session/created" || method === "session/metadataUpdated") {
			const sessionValue = objectRecord(value.session)
			if (sessionValue) {
				const session = this.rememberNativeSession(sessionValue)
				this.emit(session.directory ?? defaultCwd(), {
					type: method === "session/created" ? "session.created" : "session.updated",
					properties: { info: session, session },
				})
			}
			return true
		}
		if (method === "session/statusChanged") {
			const sessionId = String(value.sessionId ?? "")
			if (!sessionId) return true
			// Subscription replay may include historical Active status for
			// abandoned turns; live busy is seeded only from the snapshot's
			// registry-backed activeTurn (and live events after ack).
			if (this.subscriptionReplayDepth > 0 && String(value.status) === "active") {
				return true
			}
			const status = { type: String(value.status) === "active" ? "busy" : "idle" }
			if (status.type === "busy" && this.subscriptionReplayDepth === 0) {
				void this.ensureSessionSubscription(sessionId).catch(error => {
					this.emit(this.sessionDirectories.get(sessionId) ?? defaultCwd(), sessionErrorEvent(sessionId, error))
				})
			}
			this.sessionStatuses.set(sessionId, status)
			this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
				type: "session.status",
				properties: { sessionId: sessionId, status },
			})
			return true
		}
		if (method === "session/cwdChanged") {
			const sessionId = String(value.sessionId ?? "")
			const cwd = String(value.cwd ?? "")
			if (sessionId && cwd) this.sessionDirectories.set(sessionId, cwd)
			return true
		}
		if (method === "session/deleted") {
			const deletedIds = Array.isArray(value.deletedSessionIds)
				? value.deletedSessionIds.map(String)
				: [String(value.sessionId ?? "")]
			for (const sessionId of deletedIds) {
				if (!sessionId) continue
				const { directory } = this.forgetSession(sessionId)
				this.emitSessionDeleted(sessionId, directory)
			}
			return true
		}
		if (method === "session/archived") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId && value.archived === true) {
				this.sessionStatuses.set(sessionId, { type: "idle" })
			}
			return true
		}
		if (method === "session/closed") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId) {
				this.pendingQuestions.forEach((pending, requestId) => {
					if (pending.sessionId === sessionId) this.pendingQuestions.delete(requestId)
				})
				this.pendingPermissions.forEach((pending, approvalId) => {
					if (pending.sessionId === sessionId) this.pendingPermissions.delete(approvalId)
				})
				this.subscriptions.delete(sessionId)
			}
			return true
		}
		if ((method === "turn/started" || method === "turn/resumed") || method === "turn/statusChanged" || method === "turn/completed") {
			const turn = objectRecord(value.turn)
			const turnId = String(turn?.id ?? value.turnId ?? "")
			const sessionId = String(turn?.sessionId ?? this.turnSessions.get(turnId) ?? "")
			if (!sessionId) return true
			if (turnId) this.turnSessions.set(turnId, sessionId)
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			const turnStatus = String(turn?.status ?? value.status ?? "")
			const terminal = method === "turn/completed" || ["completed", "interrupted", "failed"].includes(turnStatus)
			// Historical turn/started without a matching terminal must not
			// resurrect busy after an idle/registry-empty snapshot.
			if (this.subscriptionReplayDepth > 0 && !terminal) {
				return true
			}
			if ((method === "turn/started" || method === "turn/resumed") && turnId) {
				this.activeTurnIds.set(sessionId, turnId)
				this.emit(directory, {
					type: "session.activeTurn",
					properties: { sessionId: sessionId, turnId: turnId },
				})
			}
			const status = { type: terminal ? "idle" : "busy" }
			this.sessionStatuses.set(sessionId, status)
			this.emit(directory, { type: "session.status", properties: { sessionId: sessionId, status } })
			const activityAt =
				parseTimestampMs(terminal ? turn?.completedAt : turn?.startedAt) ??
				parseTimestampMs(turn?.updatedAt) ??
				Date.now()
			// Start + terminal only: statusChanged mid-turn would spam session.updated.
			if ((method === "turn/started" || method === "turn/resumed") || terminal) {
				this.touchNativeSessionActivity(sessionId, activityAt)
			}
			if (terminal) {
				this.activeTurnIds.delete(sessionId)
				this.emit(directory, {
					type: "session.activeTurn",
					properties: { sessionId: sessionId, turnId: null },
				})
				const startedAt = this.promptStartedAtBySession.get(sessionId) ?? 0
				this.promptStartedAtBySession.delete(sessionId)
				// Native `TurnFailed` projects as `turn/completed` with `turn.error`
				// (often followed by a completed notification without error). Surface
				// the payload so Desktop can render session/assistant failure UI.
				const turnError = objectRecord(turn?.error)
				const assistantError = assistantErrorFromTurnFailure(turnStatus, turnError)
				if (assistantError) {
					this.emit(directory, {
						type: "session.error",
						properties: { sessionId: sessionId, turnId, error: assistantError },
					})
				}
				this.completeOpenAssistantMessages(sessionId, directory, startedAt, assistantError)
				this.pendingQuestions.forEach((pending, requestId) => {
					if (pending.sessionId === sessionId) this.pendingQuestions.delete(requestId)
				})
				this.pendingPermissions.forEach((pending, approvalId) => {
					if (pending.sessionId === sessionId) this.pendingPermissions.delete(approvalId)
				})
			}
			return true
		}
		if (method === "item/started" || method === "item/updated" || method === "item/completed") {
			const item = objectRecord(value.item)
			if (item) {
				this.handleNativeItemEnvelope(item, method)
				// User submissions bump activity even if turn/* was missed.
				if (method === "item/started") {
					const sessionId = String(item.sessionId ?? "")
					const itemBody = objectRecord(item.item)
					const itemType = typeof itemBody?.type === "string" ? itemBody.type : ""
					if (sessionId && itemType === "userMessage") {
						const activityAt =
							parseTimestampMs(item.updatedAt) ?? parseTimestampMs(item.createdAt) ?? Date.now()
						this.touchNativeSessionActivity(sessionId, activityAt)
					}
				}
			}
			return true
		}
		if (
			method === "context/compactionStarted" ||
			method === "context/compactionCompleted" ||
			method === "context/compactionFailed"
		) {
			const sessionId = String(value.sessionId ?? "")
			if (!sessionId) return true
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			const status =
				method === "context/compactionFailed"
					? "failed"
					: method === "context/compactionCompleted"
						? "completed"
						: "started"
			this.emit(directory, {
				type: `session.compaction.${status}`,
				properties: { sessionId: sessionId },
			})
			// Prefer item/started|completed for durable transcript markers.
			// context/compactionStarted has no itemId — only update session atom.
			return true
		}
		if (method === "item/assistantMessage/delta" || method === "item/reasoning/delta") {
			const sessionId = String(value.sessionId ?? "")
			const itemId = String(value.itemId ?? "")
			const delta = String(value.delta ?? "")
			if (sessionId && itemId && delta) {
				const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
				const itemType = method.includes("reasoning") ? "reasoning" : "assistantMessage"
				this.applyNativeTextDelta(sessionId, directory, itemId, itemType, delta)
			}
			return true
		}
		if (method === "item/commandExecution/outputDelta") {
			const sessionId = String(value.sessionId ?? "")
			const itemId = String(value.itemId ?? "")
			const delta = String(value.delta ?? "")
			if (sessionId && itemId && delta) {
				const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
				this.applyNativeCommandOutputDelta(sessionId, directory, itemId, delta)
			}
			return true
		}
		if (method === "item/tool/requestUserInput") {
			const payload = objectRecord(value.RequestUserInput) ?? value
			const request = objectRecord(payload.request) ?? {}
			const sessionId = String(request.session_id ?? request.sessionId ?? "")
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			this.handleRequestUserInput(sessionId, directory, payload)
			return true
		}
		if (method === "serverRequest/resolved") {
			const payload = objectRecord(value.ServerRequestResolved) ?? value
			const requestId = String(payload.request_id ?? payload.requestId ?? "")
			const pending = this.pendingQuestions.get(requestId)
			if (pending) {
				this.pendingQuestions.delete(requestId)
				const directory = this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd()
				this.emit(directory, { type: "question.replied", properties: { sessionId: pending.sessionId, requestId: requestId } })
			}
			return true
		}
		if (method === "permission/decision") {
			const approvalId = String(value.approvalId ?? "")
			const pending = this.pendingPermissions.get(approvalId)
			if (pending) {
				this.pendingPermissions.delete(approvalId)
				this.emit(this.sessionDirectories.get(pending.sessionId ?? "") ?? this.options.directory ?? defaultCwd(), {
					type: "permission.replied",
					properties: { sessionId: pending.sessionId ?? String(value.sessionId ?? ""), requestId: approvalId },
				})
			}
			return true
		}
		if (method === "context/usageUpdated") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId) this.emitContextUsage(sessionId, value.occupancy)
			return true
		}
		if (method === "turn/usage/updated" || method === "session/usage/updated") {
			const sessionId = String(value.sessionId ?? "")
			const usage = objectRecord(value.usage) ?? {}
			// Native usage uses the same camelCase query shape as the TUI.
			const query = objectRecord(usage.query) ?? {}
			const used = Number(query.totalTokens ?? 0)
			const size = Number(value.contextWindow ?? 0)
			if (sessionId) {
				this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
					type: "session.usage.updated",
					properties: {
						sessionId: sessionId,
						used,
						size,
						cost: 0,
					},
				})
			}
			return true
		}
		if (method === "model/queryRetrying") {
			const sessionId = String(value.sessionId ?? "")
			const error = objectRecord(value.error) ?? {}
			if (sessionId) {
				this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
					type: "turn.provider_retry_status",
					properties: {
						sessionId: sessionId,
						turnId: String(value.turnId ?? ""),
						attempt: Number(value.attempt ?? 0),
						backoffMs: Number(value.nextDelayMs ?? 0),
						provider: String(value.provider ?? ""),
						model: String(value.model ?? ""),
						phase: String(value.phase ?? "scheduled"),
						message: String(error.message ?? "Provider request retrying"),
					},
				})
			}
			return true
		}
		if (method === "workspace/changes/updated") {
			const payload = objectRecord(value.WorkspaceChangesUpdated) ?? value
			this.handleWorkspaceChangesUpdated(payload as WorkspaceChangesUpdatedNotification)
			return true
		}
		if (method === "queue/updated") {
			const sessionId = String(value.sessionId ?? "")
			if (!sessionId) return true
			const entries = parseQueueWireEntries(value.queue)
			this.emitQueueSnapshot(sessionId, entries, String(value.change ?? "updated"))
			return true
		}
		if (method === "turn/superseded") {
			const sessionId = String(value.sessionId ?? "")
			const supersededTurnId = String(value.supersededTurnId ?? "")
			if (sessionId && supersededTurnId) this.removeItemsForTurn(sessionId, supersededTurnId)
			return true
		}
		return false
	}

	/**
	 * Identity upsert of Native ItemEnvelope — no Message/Part projection.
	 * Side channels (approval, userInput, compaction status, plan todos) stay as events.
	 */
	private handleNativeItemEnvelope(envelope: Record<string, unknown>, method: string): void {
		const parsed = envelopeFromWire(envelope)
		if (!parsed) return
		const item = parsed.item
		const itemType = nativeItemType(parsed)
		const directory = this.sessionDirectories.get(parsed.sessionId) ?? this.options.directory ?? defaultCwd()
		const completed =
			method === "item/completed" ||
			parsed.state === "completed" ||
			parsed.state === "failed" ||
			parsed.state === "interrupted"

		if (itemType === "contextCompaction") {
			const envelopeState = parsed.state
			const failed = envelopeState === "failed" || item.status === "failed"
			const status = failed ? "failed" : completed || envelopeState === "completed" ? "completed" : "started"
			this.emit(directory, {
				type: `session.compaction.${status}`,
				properties: { sessionId: parsed.sessionId },
			})
			// Transcript marker is the Native envelope itself (no synthetic text Message).
			this.emitNativeItem(directory, this.storeNativeItem(parsed))
			return
		}
		if (itemType === "plan") {
			const entries = Array.isArray(item.entries) ? item.entries : []
			const mapped = expandPlanEntries(entries)
			if (mapped.length > 0) {
				this.emit(directory, {
					type: "todo.updated",
					properties: { sessionId: parsed.sessionId, todos: mapped },
				})
			}
			this.emitNativeItem(directory, this.storeNativeItem(parsed))
			return
		}
		if (itemType === "approval") {
			const approvalId = String(item.approvalId ?? "")
			if (!approvalId) return
			const availableScopes = knownApprovalScopes(
				Array.isArray(item.availableScopes) ? item.availableScopes.map(String) : undefined,
			)
			if (item.decision) {
				this.pendingPermissions.delete(approvalId)
				this.emit(directory, {
					type: "permission.replied",
					properties: { sessionId: parsed.sessionId, requestId: approvalId },
				})
				return
			}
			const existing = this.pendingPermissions.get(approvalId)
			this.pendingPermissions.set(approvalId, {
				id: existing?.id,
				method: existing?.method ?? approvalMethodFromResource(item.resource),
				sessionId: parsed.sessionId,
				options: existing?.options ?? [],
				availableScopes,
				native: true,
			})
			this.emitPermissionAsked(parsed.sessionId, directory, approvalId, item, availableScopes)
			return
		}
		if (itemType === "userInputRequest") {
			const requestId = String(item.requestId ?? "")
			if (!requestId) return
			const pending = this.pendingQuestions.get(requestId)
			if (item.answers || completed) {
				this.pendingQuestions.delete(requestId)
				if (pending) {
					this.emit(directory, {
						type: "question.replied",
						properties: { sessionId: pending.sessionId || parsed.sessionId, requestId },
					})
				}
			} else {
				const questions = (Array.isArray(item.questions) ? item.questions : []).map(questionInfoFromNative)
				this.pendingQuestions.set(requestId, {
					id: pending?.id,
					method: pending?.method ?? "userInput/request",
					sessionId: parsed.sessionId,
					questions,
				})
				this.emit(directory, {
					type: "question.asked",
					properties: { id: requestId, sessionId: parsed.sessionId, questions },
				})
			}
			return
		}

		if (item.callId) this.nativeItemCallIds.set(parsed.id, String(item.callId))
		this.emitNativeItem(directory, this.storeNativeItem(parsed))
	}

	private storeNativeItem(envelope: NativeItemEnvelope): NativeItemEnvelope {
		let byId = this.items.get(envelope.sessionId)
		if (!byId) {
			byId = new Map()
			this.items.set(envelope.sessionId, byId)
		}
		const merged = mergeNativeEnvelope(byId.get(envelope.id), envelope)
		byId.set(envelope.id, merged)
		return merged
	}

	private emitNativeItem(directory: string, envelope: NativeItemEnvelope): void {
		this.emit(directory, {
			type: "item.updated",
			properties: { info: envelope },
		})
	}

	private applyNativeTextDelta(
		sessionId: string,
		directory: string,
		itemId: string,
		itemType: "assistantMessage" | "reasoning",
		delta: string,
	): void {
		const existing = this.items.get(sessionId)?.get(itemId)
		const prevText =
			typeof existing?.item?.text === "string" ? existing.item.text : ""
		const next: NativeItemEnvelope = existing
			? {
					...existing,
					updatedAt: existing.updatedAt || new Date().toISOString(),
					item: { ...existing.item, type: itemType, text: prevText + delta },
				}
			: {
					id: itemId,
					sessionId,
					turnId: "",
					seq: 0,
					revision: 0,
					createdAt: new Date().toISOString(),
					updatedAt: new Date().toISOString(),
					state: "running",
					item: { type: itemType, text: delta },
				}
		this.emitNativeItem(directory, this.storeNativeItem(next))
	}

	private applyNativeCommandOutputDelta(
		sessionId: string,
		directory: string,
		itemId: string,
		delta: string,
	): void {
		const existing = this.items.get(sessionId)?.get(itemId)
		const prevOutput =
			typeof existing?.item?.output === "string"
				? existing.item.output
				: typeof existing?.item?.displayContent === "string"
					? existing.item.displayContent
					: ""
		const callId = this.nativeItemCallIds.get(itemId) ?? (existing?.item?.callId ? String(existing.item.callId) : itemId)
		const next: NativeItemEnvelope = existing
			? {
					...existing,
					item: {
						...existing.item,
						type: existing.item.type ?? "commandExecution",
						callId,
						output: prevOutput + delta,
					},
				}
			: {
					id: itemId,
					sessionId,
					turnId: "",
					seq: 0,
					revision: 0,
					createdAt: new Date().toISOString(),
					updatedAt: new Date().toISOString(),
					state: "running",
					item: { type: "commandExecution", callId, output: delta, command: "", isError: false },
				}
		this.emitNativeItem(directory, this.storeNativeItem(next))
	}

	private removeItemsForTurn(sessionId: string, turnId: string): void {
		this.loadedSessionLimits.delete(sessionId)
		this.historyCursors.delete(sessionId)
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		const byId = this.items.get(sessionId)
		if (!byId) return
		for (const [itemId, envelope] of [...byId.entries()]) {
			if (envelope.turnId !== turnId) continue
			byId.delete(itemId)
			this.nativeItemCallIds.delete(itemId)
			this.emit(directory, {
				type: "item.removed",
				properties: { sessionId, itemId },
			})
		}
	}

	private applySubscriptionSessionSnapshot(snapshot: Record<string, unknown>): boolean {
		const data = objectRecord(snapshot.data)
		const session = objectRecord(data?.session)
		if (session) this.rememberNativeSession(session)
		const sessionId = String(session?.id ?? snapshot.sessionId ?? "")
		if (!sessionId) return false
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		if (data?.queue !== undefined) {
			this.emitQueueSnapshot(sessionId, parseQueueWireEntries(data.queue), "sync")
		}
		const activeTurn = objectRecord(data?.active_turn ?? data?.activeTurn)
		if (activeTurn?.id) {
			const turnId = String(activeTurn.id)
			this.activeTurnIds.set(sessionId, turnId)
			this.sessionStatuses.set(sessionId, { type: "busy" })
			this.emit(directory, {
				type: "session.status",
				properties: { sessionId: sessionId, status: { type: "busy" } },
			})
			this.emit(directory, {
				type: "session.activeTurn",
				properties: { sessionId: sessionId, turnId: turnId },
			})
			return true
		}
		// Registry-empty snapshot: durable InProgress is not live work.
		this.activeTurnIds.delete(sessionId)
		this.sessionStatuses.set(sessionId, { type: "idle" })
		this.emit(directory, {
			type: "session.status",
			properties: { sessionId: sessionId, status: { type: "idle" } },
		})
		this.emit(directory, {
			type: "session.activeTurn",
			properties: { sessionId: sessionId, turnId: null },
		})
		return false
	}

	private async ensureSessionSubscription(sessionId: string): Promise<void> {
		if (this.subscriptions.has(sessionId)) return
		const pending = this.subscriptionLoads.get(sessionId)
		if (pending) return pending
		const load = this.subscribeSession(sessionId)
		this.subscriptionLoads.set(sessionId, load)
		try { await load } finally {
			if (this.subscriptionLoads.get(sessionId) === load) this.subscriptionLoads.delete(sessionId)
		}
	}

	private async subscribeSession(sessionId: string): Promise<void> {
		if (this.subscriptions.has(sessionId)) return
		let result: {
			subscriptionId: string
			snapshots?: Array<Record<string, unknown>>
			replay?: Array<Record<string, unknown>>
			cursors?: Array<{ streamId: string; seq: number }>
			pendingControlRequests?: Array<Record<string, unknown>>
			recoverySnapshots?: Array<{ item: Record<string, unknown> }>
		}
		try {
			result = (await this.requestCanonical("subscription/create", {
				selectors: [{ kind: "session", sessionId }],
				includeSnapshot: true,
				replay: "snapshotOnly",
				after: [],
			})) as typeof result
		} catch (error) {
			if (isSessionNotFoundError(error)) {
				this.dropMissingSession(sessionId)
				return
			}
			throw error
		}
		const cursors = result.cursors ?? []
		this.subscriptions.set(sessionId, { subscriptionId: result.subscriptionId, cursors })
		this.subscriptionCursors.set(sessionId, cursors)
		let snapshotHasActiveTurn = false
		let sawRegistryEmptySnapshot = false
		for (const snapshot of result.snapshots ?? []) {
			const hasActiveTurn = this.applySubscriptionSessionSnapshot(snapshot)
			if (hasActiveTurn) {
				snapshotHasActiveTurn = true
			} else if (objectRecord(objectRecord(snapshot.data)?.session)?.id || snapshot.sessionId) {
				sawRegistryEmptySnapshot = true
			}
		}
		this.subscriptionReplayDepth += 1
		try {
			for (const envelope of result.replay ?? []) {
				const notification = objectRecord(envelope.notification)
				if (notification && typeof notification.method === "string") {
					this.handleNativeNotification(notification.method, notification.params)
				}
			}
			for (const recovery of result.recoverySnapshots ?? []) {
				this.handleNativeItemEnvelope(recovery.item, nativeItemNotificationMethod(recovery.item))
			}
			for (const pending of result.pendingControlRequests ?? []) {
				const item = objectRecord(pending.item)
				if (item) this.handleNativeItemEnvelope(item, "item/started")
			}
		} finally {
			this.subscriptionReplayDepth -= 1
		}
		if (sawRegistryEmptySnapshot && !snapshotHasActiveTurn) {
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			this.activeTurnIds.delete(sessionId)
			this.sessionStatuses.set(sessionId, { type: "idle" })
			this.emit(directory, {
				type: "session.status",
				properties: { sessionId: sessionId, status: { type: "idle" } },
			})
			this.emit(directory, {
				type: "session.activeTurn",
				properties: { sessionId: sessionId, turnId: null },
			})
		}
		if (cursors.length > 0) {
			await this.requestCanonical("subscription/ack", { subscriptionId: result.subscriptionId, cursors })
		}
	}

	private async ensureKnownSessionSubscriptions(): Promise<void> {
		const sessions = this.options.directory ? [] : await this.listSessions()
		const directories = this.options.directory ? [this.options.directory] : [...new Set(sessions.map(s => s.directory).filter(Boolean))]
		for (const cwd of directories) {
			let load = this.directorySubscriptions.get(cwd)
			if (!load) {
				load = this.subscribeDirectory(cwd)
				this.directorySubscriptions.set(cwd, load)
			}
			try { await load } catch (error) {
				if (this.directorySubscriptions.get(cwd) === load) this.directorySubscriptions.delete(cwd)
				throw error
			}
		}
		for (const session of this.sessions.values()) {
			if (this.options.directory && session.directory !== this.options.directory) continue
			if (this.sessionStatuses.get(session.id)?.type === "busy") await this.ensureSessionSubscription(session.id)
		}
	}

	private async subscribeDirectory(cwd: string): Promise<void> {
		const result = await this.requestCanonical("subscription/create", {
			selectors: [{ kind: "sessionsByCwd", cwd }], includeSnapshot: true, replay: "snapshotOnly", after: [],
		}) as { subscriptionId: string; snapshots?: Array<Record<string, unknown>>; cursors?: Array<{ streamId: string; seq: number }> }
		for (const snapshot of result.snapshots ?? []) {
			const data = objectRecord(snapshot.data)
			for (const session of (data?.sessions ?? []) as Array<Record<string, unknown>>) {
				// Folder rosters project the lightweight index. Preserve richer
				// canonical model/settings already obtained from resume or reads.
				const existing = this.sessions.get(String(session.id ?? ""))
				this.rememberNativeSession({
					...session,
					model: existing?.model ?? session.model,
					settings: existing?.settings ?? session.settings,
				})
			}
		}
		if (result.cursors?.length) await this.requestCanonical("subscription/ack", { subscriptionId: result.subscriptionId, cursors: result.cursors })
	}

	private emitPermissionAsked(
		sessionId: string,
		directory: string,
		approvalId: string,
		item: Record<string, unknown>,
		availableScopes: PermissionResponse[],
	): void {
		const target = objectRecord(item.target)
		const targetKind = String(target?.kind ?? "")
		const answerable = this.pendingPermissions.get(approvalId)?.id !== undefined
		this.emit(directory, {
			type: "permission.asked",
			properties: {
				id: approvalId,
				sessionId: sessionId,
				permission: String(item.actionSummary ?? "Agent requested permission"),
				metadata: {
					tool: item.resource,
					command: targetKind === "command" ? target?.command : undefined,
					path: targetKind === "path" ? target?.path : undefined,
					host: targetKind === "host" ? target?.host : undefined,
					justification: item.justification,
					resource: item.resource,
					target: target?.command ?? target?.path ?? target?.host,
					availableScopes,
					commandPattern: item.commandPattern,
					commandPrefix: item.commandPrefix,
					answerable,
				},
			},
		})
	}

	private async respondToPermission(
		permissionId: string,
		response: PermissionResponse,
	): Promise<void> {
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		const pending = this.pendingPermissions.get(permissionId)
		if (!pending) return
		if (pending.id === undefined) {
			throw new Error("Permission request is not answerable yet; wait for the connection to restore")
		}
		this.pendingPermissions.delete(permissionId)
		const scopes = knownApprovalScopes(pending.availableScopes)
		const requestedScope = response === "always"
			? ["commandPrefixPersist", "commandPrefix", "pathPrefix", "host", "tool", "session", "turn", "once"]
				.find((candidate) => scopes.includes(candidate as PermissionResponse)) ?? "once"
			: response === "reject" ? "once" : response
		const scope = scopes.includes(requestedScope as PermissionResponse) ? requestedScope : "once"
		const result = assertValidProtocolPayload({
			method: pending.method,
			direction: "outgoingResponse",
			payload: {
				requestId: permissionId,
				decision: {
					decision: response === "reject" ? "denied" : "approved",
					scope,
					decisionSource: "user",
					decidedAt: new Date().toISOString(),
				},
			},
		})
		await this.transport.respond(pending.id, result)
		this.emit(
			this.sessionDirectories.get(pending.sessionId ?? "") ?? this.options.directory ?? defaultCwd(),
			{
				type: "permission.replied",
				properties: {
					sessionId: pending.sessionId ?? "",
					requestId: permissionId,
				},
			},
		)
	}

	private async respondToQuestion(
		requestId: string,
		answers: QuestionAnswer[],
		eventType: "question.replied" | "question.rejected",
	): Promise<void> {
		const pending = this.pendingQuestions.get(requestId)
		if (!pending) return
		const responseAnswers: Record<string, { answers: string[] }> = {}
		pending.questions.forEach((question, index) => {
			const rawAnswer = answers[index]
			const answerValues = Array.isArray(rawAnswer)
				? rawAnswer.map(String)
				: rawAnswer === undefined || rawAnswer === null
					? []
					: [String(rawAnswer)]
			responseAnswers[question.id] = { answers: answerValues }
		})
		if (pending.id === undefined) {
			throw new Error("pending user-input request has no JSON-RPC request id")
		}
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		const result = assertValidProtocolPayload({
			method: pending.method ?? "userInput/request",
			direction: "outgoingResponse",
			payload: {
				requestId,
				answers: responseAnswers,
			},
		})
		await this.transport.respond(pending.id, result)
		this.pendingQuestions.delete(requestId)
		this.emit(this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd(), {
			type: eventType,
			properties: {
				sessionId: pending.sessionId,
				requestId: requestId,
			},
		})
	}

	private forgetSession(
		sessionId: string,
		fallbackDirectory = this.options.directory ?? defaultCwd(),
	): { directory: string; known: boolean } {
		const session = this.sessions.get(sessionId)
		const directory = this.sessionDirectories.get(sessionId) ?? session?.directory ?? fallbackDirectory
		const known =
			this.sessions.has(sessionId) ||
			this.sessionStatuses.has(sessionId) ||
			this.sessionDirectories.has(sessionId) ||
			this.loadedSessionLimits.has(sessionId) ||
			this.items.has(sessionId)
		this.sessions.delete(sessionId)
		this.sessionStatuses.delete(sessionId)
		this.promptStartedAtBySession.delete(sessionId)
		this.sessionDirectories.delete(sessionId)
		this.loadedSessionLimits.delete(sessionId)
		this.historyCursors.delete(sessionId)
		this.resumedSessions.delete(sessionId)
		const sessionItems = this.items.get(sessionId)
		if (sessionItems) {
			for (const itemId of sessionItems.keys()) {
				this.nativeItemCallIds.delete(itemId)
			}
		}
		this.items.delete(sessionId)
		this.activeTurnIds.delete(sessionId)
		this.queueEntriesBySession.delete(sessionId)
		return { directory, known }
	}

	/** Mark in-flight Native assistant/reasoning/tool items completed when a turn ends. */
	private completeOpenAssistantMessages(
		sessionId: string,
		directory: string,
		_promptStartedAt: number,
		error?: { name: string; data: Record<string, unknown> },
	): void {
		const byId = this.items.get(sessionId)
		if (!byId) return
		for (const [itemId, envelope] of [...byId.entries()]) {
			if (envelope.state === "completed" || envelope.state === "failed" || envelope.state === "interrupted") {
				continue
			}
			const itemType = String(envelope.item?.type ?? "")
			if (
				itemType !== "assistantMessage" &&
				itemType !== "reasoning" &&
				itemType !== "toolCall" &&
				itemType !== "commandExecution" &&
				itemType !== "hostedToolCall"
			) {
				continue
			}
			const updated: NativeItemEnvelope = {
				...envelope,
				state: error ? "failed" : "completed",
				updatedAt: envelope.updatedAt || new Date().toISOString(),
				item: error ? { ...envelope.item, error } : envelope.item,
			}
			byId.set(itemId, updated)
			this.emitNativeItem(directory, updated)
		}
	}

	private emitSessionDeleted(sessionId: string, directory: string): void {
		this.emit(directory, {
			type: "session.deleted",
			properties: { info: { id: sessionId, directory } },
		})
	}

	private handleWorkspaceChangesUpdated(
		payload: WorkspaceChangesUpdatedNotification,
		directory?: string,
	): void {
		if (!payload.sessionId) return
		const event: WorkspaceChangesUpdatedEventProperties = {
			sessionId: payload.sessionId,
			turnId: payload.turnId,
			scope: payload.scope,
			status: payload.status,
			coverage: payload.coverage,
			changeSetStatus: payload.changeSetStatus,
			stats: {
				filesChanged: numberFromProtocol(payload.stats.filesChanged),
				additions: numberFromProtocol(payload.stats.additions),
				deletions: numberFromProtocol(payload.stats.deletions),
			},
			version: numberFromProtocol(payload.version),
			generatedAt: payload.generatedAt,
		}
		const emitDirectory =
			directory ?? this.sessionDirectories.get(event.sessionId) ?? this.options.directory ?? defaultCwd()
		this.emit(emitDirectory, {
			type: "workspace.changes.updated",
			properties: event,
		})
	}

	private handleRequestUserInput(
		sessionId: string,
		directory: string,
		payload: Record<string, unknown>,
	): void {
		const request = (payload.request ?? {}) as Record<string, unknown>
		const requestId = String(request.request_id ?? request.requestId ?? "")
		if (!requestId) return
		const requestSessionId = String(request.session_id ?? request.sessionId ?? sessionId)
		const rawQuestions = Array.isArray(payload.questions) ? payload.questions : []
		const questions = rawQuestions.map(questionInfoFromNative)
		const existing = this.pendingQuestions.get(requestId)
		this.pendingQuestions.set(requestId, {
			id: existing?.id,
			method: existing?.method,
			sessionId: requestSessionId,
			questions,
		})
		this.emit(directory, {
			type: "question.asked",
			properties: {
				id: requestId,
				sessionId: requestSessionId,
				questions,
			},
		})
	}

	private nextEventTime(): number {
		const now = Date.now()
		const eventTime = Math.max(now, this.lastEventTime + 1)
		this.lastEventTime = eventTime
		return eventTime
	}

	private queueEntriesForSession(sessionId: string): QueueWireEntry[] {
		return this.queueEntriesBySession.get(sessionId) ?? []
	}

	private emitQueueSnapshot(sessionId: string, entries: QueueWireEntry[], change: string): void {
		this.queueEntriesBySession.set(sessionId, entries)
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		this.emit(directory, {
			type: "session.queue.updated",
			properties: {
				sessionId: sessionId,
				change,
				entries,
			},
		})
	}

	private readonly sessionsNeedingResume = new Set<string>()

	private async pushSessionQueue(params: {
		sessionId: string
		parts: PromptPartInput[]
		collaborationMode?: string
	}): Promise<PromptAsyncOutcome> {
		if (this.sessionsNeedingResume.has(params.sessionId)) {
			await this.ensureSessionReady(params.sessionId)
		}
		if (params.collaborationMode) {
			const settingsPatch: SessionSettingsPatch = {
				mode: params.collaborationMode,
			}
			await this.enqueueSessionSettings(params.sessionId, settingsPatch)
		}
		await this.ensureSessionSubscription(params.sessionId)
		const request = {
			sessionId: params.sessionId,
			input: userInputsFromPromptParts(params.parts),
			idempotencyKey: crypto.randomUUID(),
		}
		let result: Record<string, unknown>
		try {
			result = await this.requestCanonical("session/queue/push", request) as Record<string, unknown>
		} catch (error) {
			if (!isSessionNotFoundError(error)) throw error
			// An evicted actor or a missed transport-close event can invalidate
			// the client cache. Restore once, then retry with the same input/key.
			this.loadedSessionLimits.delete(params.sessionId)
			this.historyCursors.delete(params.sessionId)
			this.resumedSessions.delete(params.sessionId)
			this.subscriptions.delete(params.sessionId)
			await this.ensureSessionReady(params.sessionId)
			result = await this.requestCanonical("session/queue/push", request) as Record<string, unknown>
		}
		const outcome = String(result.outcome ?? "")
		if (outcome === "queued" || result.entry) {
			const entry = objectRecord(result.entry) ?? result
			const entries = parseQueueWireEntries([entry])
			if (entries[0]) {
				const current = this.queueEntriesForSession(params.sessionId)
				const merged = [...current.filter((item) => item.queueItemId !== entries[0].queueItemId), entries[0]]
				this.emitQueueSnapshot(
					params.sessionId,
					merged.sort((left, right) => left.position - right.position),
					"added",
				)
				return { outcome: "queued", queueItemId: entries[0].queueItemId }
			}
			return { outcome: "queued", queueItemId: String(entry.queueItemId ?? "") }
		}
		const turn = objectRecord(result.turn)
		if (turn?.id) {
			this.activeTurnIds.set(params.sessionId, String(turn.id))
			const directory = this.sessionDirectories.get(params.sessionId) ?? this.options.directory ?? defaultCwd()
			this.emit(directory, {
				type: "session.activeTurn",
				properties: { sessionId: params.sessionId, turnId: String(turn.id) },
			})
		}
		return { outcome: "started", ...(turn?.id ? { turnId: String(turn.id) } : {}) }
	}

	private rememberConfigOptions(
		sessionId: string,
		directory: string,
		configOptions?: SessionConfigOption[],
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsBySession.set(sessionId, configOptions)
		this.rememberDirectoryConfigOptions(directory, configOptions)
	}

	private rememberDirectoryConfigOptions(
		directory: string,
		configOptions?: SessionConfigOption[] | null,
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsByDirectory.set(directory, configOptions)
	}

	private async enqueueSessionSettings(
		sessionId: string,
		patch: SessionSettingsPatch,
	): Promise<Session | undefined> {
		const normalizedPatch: SessionSettingsPatch = {}
		if (typeof patch.modelID === "string" && patch.modelID.length > 0) {
			normalizedPatch.modelID = patch.modelID
		}
		if (typeof patch.reasoningEffort === "string" && patch.reasoningEffort.length > 0) {
			normalizedPatch.reasoningEffort = patch.reasoningEffort
		}
		if (typeof patch.mode === "string" && patch.mode.length > 0) {
			normalizedPatch.mode = patch.mode
		}
		if (typeof patch.permissionProfile === "string" && patch.permissionProfile.length > 0) {
			normalizedPatch.permissionProfile = patch.permissionProfile
		}
		if (Object.keys(normalizedPatch).length === 0) return this.sessions.get(sessionId)

		let queue = this.sessionSettingsQueues.get(sessionId)
		if (!queue) {
			queue = { pending: null, waiters: [], running: null, paused: false }
			this.sessionSettingsQueues.set(sessionId, queue)
		}
		queue.pending = mergeSessionSettingsPatch(queue.pending, normalizedPatch)
		queue.paused = false
		const result = new Promise<Session | undefined>((resolve, reject) => {
			queue.waiters.push({ resolve, reject })
		})
		this.startSessionSettingsDrain(sessionId, queue)
		return result
	}

	private startSessionSettingsDrain(sessionId: string, queue: SessionSettingsQueue): void {
		if (queue.running || queue.paused || !queue.pending) return
		const running = this.drainSessionSettings(sessionId, queue)
		queue.running = running
	}

	private async drainSessionSettings(
		sessionId: string,
		queue: SessionSettingsQueue,
	): Promise<void> {
		try {
			while (!queue.paused && queue.pending) {
				const patch = queue.pending
				const waiters = queue.waiters
				queue.pending = null
				queue.waiters = []
				try {
					const session = await this.persistSessionSettingsWithRetry(sessionId, patch)
					if (!queue.pending) {
						const directory =
							session?.directory ??
							this.sessionDirectories.get(sessionId) ??
							this.options.directory ??
							defaultCwd()
						this.emit(directory, {
							type: "session.updated",
							properties: { info: session, session },
						})
					}
					for (const waiter of waiters) waiter.resolve(session)
				} catch (error) {
					// Keep the failed patch, merged ahead of any newer selection, so
					// a manual retry or the next selection cannot lose a field.
					queue.pending = mergeSessionSettingsPatch(patch, queue.pending ?? {})
					for (const waiter of waiters) waiter.reject(error)
					queue.paused = true
				}
			}
		} finally {
			queue.running = null
			if (!queue.paused && queue.pending) this.startSessionSettingsDrain(sessionId, queue)
		}
	}

	private async retrySessionSettings(sessionId: string): Promise<Session | undefined> {
		const queue = this.sessionSettingsQueues.get(sessionId)
		if (!queue?.pending) return this.sessions.get(sessionId)
		queue.paused = false
		const result = new Promise<Session | undefined>((resolve, reject) => {
			queue.waiters.push({ resolve, reject })
		})
		this.startSessionSettingsDrain(sessionId, queue)
		return result
	}

	private async persistSessionSettingsWithRetry(
		sessionId: string,
		patch: SessionSettingsPatch,
	): Promise<Session> {
		for (let retry = 0; ; retry += 1) {
			try {
				const update: Record<string, unknown> = {
					sessionId,
					expectedVersion: 0,
				}
				if (patch.modelID) update.model = { provider: "", model: patch.modelID }
				const settings: Record<string, string> = {}
				if (patch.reasoningEffort) settings.reasoningEffort = patch.reasoningEffort
				if (patch.mode) settings.mode = patch.mode
				if (patch.permissionProfile) settings.permissionProfile = patch.permissionProfile
				if (Object.keys(settings).length > 0) update.settings = settings

				const result = (await this.requestCanonical("session/metadata/update", update)) as {
					session?: Record<string, unknown>
				}
				if (!result.session) throw new Error("session/metadata/update returned no session")
				return this.rememberNativeSession(result.session)
			} catch (error) {
				if (isSessionNotFoundError(error)) {
					this.dropMissingSession(sessionId)
					throw error
				}
				const delay = SESSION_SETTINGS_RETRY_DELAYS_MS[retry]
				if (delay === undefined || !isTransientSessionSettingsError(error)) throw error
				await waitForSessionSettingsRetry(delay)
			}
		}
	}

	private async setDefaultConfigOption(
		configId: string,
		value: string,
	): Promise<SessionConfigOption[]> {
		await this.ensureInitialized()
		// Canonical model/preferences/write (ratified #12): configId maps to
		// the patch fields; the result converts back to the select shape the
		// config UI renders.
		const patch: Record<string, string> = {}
		if (configId === "model") patch.model = value
		else if (configId === "thought_level") patch.reasoningEffort = value
		else throw new Error(`unknown model config option '${configId}'`)
		await this.model.preferences.write({ patch })
		return this.currentConfigOptions()
	}

	private cachedConfigOptions(): SessionConfigOption[] | undefined {
		if (this.options.directory) {
			const byDirectory = this.configOptionsByDirectory.get(this.options.directory)
			if (byDirectory) return byDirectory
		}
		return this.configOptionsBySession.values().next().value
	}

	private currentConfigOptions(): SessionConfigOption[] {
		return this.cachedConfigOptions() ?? []
	}

	invalidateConfigOptionCaches(): void {
		this.configOptionsBySession.clear()
		this.configOptionsByDirectory.clear()
	}

	private async ensureCurrentConfigOptions(): Promise<SessionConfigOption[]> {
		const cached = this.cachedConfigOptions()
		if (cached) return cached

		await this.ensureInitialized()
		const directory = this.options.directory ?? defaultCwd()
		// Canonical model/preferences/read (ratified #12).
		const params: Record<string, unknown> = this.options.directory ? { cwd: this.options.directory } : {}
		const result = (await this.requestCanonical("model/preferences/read", params)) as {
			preferences?: ModelPreferencesWire
		}
		this.rememberDirectoryConfigOptions(directory, sessionConfigOptionsFromModelPreferences(result.preferences ?? {}))
		return this.currentConfigOptions()
	}

	private emitContextUsage(sessionId: string, occupancyValue: unknown): boolean {
		const occupancy = contextOccupancyFromProtocol(occupancyValue)
		if (!occupancy) return false
		this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
			type: "context.usage.updated",
			properties: { sessionId: sessionId, occupancy },
		})
		return true
	}

	private emit(directory: string, payload: Event): void {
		this.events.push({ directory, payload })
	}

	private emitProtocolValidationError(method: string, payload: unknown, error: unknown): void {
		const sessionId = sessionIdFromPayload(payload) ?? "protocol"
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		const reason =
			error instanceof ProtocolValidationError
				? error
				: new ProtocolValidationError({
						method,
						direction: "incomingNotification",
						payload,
						message: error instanceof Error ? error.message : String(error),
					})
		this.emit(directory, sessionErrorEvent(sessionId, reason))
	}
}

export type DevoClient = any

export function createDevoClient(options: CreateDevoClientOptions = {}): DevoClient {
	return new NativeClient(options)
}

const DROPPED_REPLAY_LOG_INTERVAL_MS = 60_000
let lastDroppedReplayLogAt = 0

/**
 * Root-cause capture for forward-compatible replay handling: one line per
 * minute max, carrying the offending envelope itself (Electron's log
 * formatter renders nested objects as `[Object]`, so it must be stringified
 * here). An empty method string means the envelope had no parseable method.
 */
function reportDroppedReplayEnvelopes(method: string, dropped: Array<unknown>): void {
	const now = Date.now()
	if (now - lastDroppedReplayLogAt < DROPPED_REPLAY_LOG_INTERVAL_MS) return
	lastDroppedReplayLogAt = now
	const details = dropped
		.slice(0, 3)
		.map((envelope) => {
			let text: string
			try {
				text = JSON.stringify(envelope) ?? String(envelope)
			} catch {
				text = String(envelope)
			}
			return text.slice(0, 800)
		})
		.join(" | ")
	console.warn(
		`[devo-sdk] dropped ${dropped.length} replay envelope(s) with unknown notification method from ${method}: ${details}`,
	)
}

function sessionIdFromPayload(payload: unknown): string | null {
	if (!payload || typeof payload !== "object") return null
	const value = payload as Record<string, unknown>
	for (const key of ["sessionId", "session_id"]) {
		if (typeof value[key] === "string") return value[key] as string
	}
	return null
}
