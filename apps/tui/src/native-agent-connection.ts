/**
 * Native AgentConnection — Devo server transport for InteractiveMode.
 *
 * Emits InteractiveMode-shaped session_event payloads only. Native turn/item vocabulary
 * is projected inside this adapter (see native-to-pi/events.ts).
 */

import { ModelCatalogRefresh } from "./model-catalog-refresh.js";
import { randomUUID } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type {
  AgentConnection,
  AgentConnectionEvent,
  AgentConnectionEventListener,
  AgentConnectionBeforeSessionInvalidateListener,
  AgentConnectionExtensionUiResponse,
  AgentConnectionModel,
  AgentConnectionModelCatalog,
  AgentConnectionNewSessionOptions,
  AgentConnectionPromptOptions,
  AgentConnectionQueueState,
  AgentConnectionResourceSnapshot,
  AgentConnectionSnapshot,
  AgentConnectionState,
  AgentConnectionSwitchSessionOptions,
  AgentConnectionForkOptions,
  AgentConnectionSlashCommand,
  AgentConnectionSessionContext,
  AgentConnectionSessionTreeNode,
  AgentConnectionSavedSessionInfo,
  AgentConnectionSavedSessionScope,
  AgentConnectionSessionListCallbacks,
  AgentConnectionToolDefinition,
} from "@earendil-works/pi-coding-agent/agent-connection";
import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { ContextTreeNode } from "../lib/coding-agent/src/core/context-tree.js";
import type { BashResult } from "../lib/coding-agent/src/core/bash-executor.js";
import { measureAsync } from "@earendil-works/pi-coding-agent/timings";
import { StdioJsonRpc, type JsonRpcId } from "./stdio-jsonrpc.js";
import {
  abortUiSessionEvents,
  createStreamState,
  approvalResolutionFromNotification,
  nativeNotificationToPiEvents,
  type StreamState,
} from "./native-to-pi/events.js";
import { fileChangeSummariesFromWorkspaceViews } from "./native-to-pi/tools.js";
import { formatNativeToolLabel } from "./native-to-pi/tool-label.js";
import {
  applyFailedTurnsToAgentMessages,
  emptyGoalState,
  modalitiesToInput,
  occupancyToContextUsage,
  nativeItemsToAgentMessages,
} from "./native-to-pi/messages.js";
import { subscribeAgentRoster as subscribeNativeAgentRoster } from "./native-agents-view-bridge.js";
import { resolveDevoHome } from "./host.js";
import { SessionImportFileNotFoundError } from "@earendil-works/pi-coding-agent";
import {
  applyGoalFromWire,
  countMessageStats,
  credentialSetParamsFromAuth,
  mapMcpToExtensions,
  mapSkillSourceInfo,
  mapSkillsToResourceSkills,
  newAgentMessageReceipt,
  parseGoalSlash,
  parsePermissionsSlash,
  parseQueueWireEntries,
  parseTracesSlash,
  queuePreviewText,
  workspacePathsFromChanges,
  type AuthCredentialLike,
  type QueueWireEntry,
} from "./native-connection-ops.js";
import {
  classifyNativeTrafficLine,
  formatTracePreviewRecord,
  type NativeTrafficLog,
} from "./native-traffic-log.js";

export const DEFAULT_NATIVE_API_VERSION = "2026-09-01";

export const STRIP_VENDOR_COMMANDS = new Set([
  "/update",
  "/changelog",
  "/logs",
  "/autonomous",
  "/share",
  "/rlm-max-depth",
  "/fast",
]);

/**
 * Only Devo-specific extras. InteractiveMode builtins (model, effort, compact, new, …)
 * come from InteractiveMode's BUILTIN_SLASH_COMMANDS. Listing them again as
 * source:"prompt" made /skills|/compact look real while falling through to
 * turn/start as plain user text.
 *
 * `/traces` is listed here because the host (see `host.ts`
 * `excludedBuiltinCommands`) takes it away from the vendored upload-oriented handler
 * and serves the local native traffic traces instead. `/update` is excluded so Devo never offers upstream self-update.
 */
const DEVO_SLASH_COMMAND_SPECS: ReadonlyArray<{ name: string; description: string }> = [
  // `/permissions` is an InteractiveMode builtin (see CANONICAL_BUILTIN_SLASH_COMMANDS);
  // listing it here too trips the builtin-conflict warning in autocomplete.
  { name: "/traces", description: "Show, toggle, or preview native protocol traces" },
  { name: "/plan", description: "Switch session to plan mode (read-only mutations denied)" },
  { name: "/build", description: "Switch session to build mode (default)" },
];

export const DEVO_SLASH_COMMANDS: AgentConnectionSlashCommand[] = DEVO_SLASH_COMMAND_SPECS.map(
  ({ name, description }) => ({
    name: name.slice(1),
    description,
    source: "extension" as const,
    sourceInfo: {
      path: name,
      source: "devo",
      scope: "user" as const,
      origin: "top-level" as const,
    },
  }),
);

function newIdempotencyKey(prefix: string): string {
  return `${prefix}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 10)}`;
}

function textInput(message: string) {
  return [{ type: "text", text: message }];
}

/**
 * Map InteractiveMode / pi-ai `ImageContent` (`{ type, data, mimeType }`) onto
 * Native `UserInput::Image` (`{ type, uri, mimeType? }`). Spreading the raw
 * ImageContent leaves `data` instead of `uri`, which fails turn/start decode.
 */
export function nativeImageUserInputs(images: unknown[] | undefined): Array<Record<string, unknown>> {
  if (!images?.length) return [];
  const out: Array<Record<string, unknown>> = [];
  for (const raw of images) {
    if (!raw || typeof raw !== "object") continue;
    const img = raw as Record<string, unknown>;
    if (typeof img.uri === "string" && img.uri.length > 0) {
      out.push({
        type: "image",
        uri: img.uri,
        ...(typeof img.mimeType === "string" ? { mimeType: img.mimeType } : {}),
        ...(img.detail != null ? { detail: img.detail } : {}),
      });
      continue;
    }
    if (typeof img.path === "string" && img.path.length > 0) {
      out.push({
        type: "localImage",
        path: img.path,
        ...(img.detail != null ? { detail: img.detail } : {}),
      });
      continue;
    }
    const data = typeof img.data === "string" ? img.data : null;
    if (!data) continue;
    const mimeType =
      typeof img.mimeType === "string" && img.mimeType.length > 0 ? img.mimeType : "image/png";
    out.push({
      type: "image",
      uri: `data:${mimeType};base64,${data}`,
      mimeType,
      ...(img.detail != null ? { detail: img.detail } : {}),
    });
  }
  return out;
}

function promptInput(message: string, images?: unknown[]) {
  return [...textInput(message), ...nativeImageUserInputs(images)];
}

function emptySessionActions() {
  return {
    queuedCount: 0,
    steering: [] as string[],
    followUps: [] as string[],
  };
}

function emptyResourceSnapshot(): AgentConnectionResourceSnapshot {
  return {
    contextFiles: [],
    skills: [],
    prompts: [],
    extensions: [],
    themes: [],
    diagnostics: { skills: [], prompts: [], extensions: [], themes: [] },
  };
}

function emptyContextUsage() {
  return undefined;
}

type ConnectOptions = {
  writeLine: (line: string) => void;
  cwd?: string;
  nativeApiVersion?: string;
  /** Optional native traffic trace sink, driven by `/traces` and DEVO_PROTOCOL_TRACE. */
  trafficLog?: NativeTrafficLog;
};

export class NativeAgentConnection implements AgentConnection {
  readonly rpc: StdioJsonRpc;
  private cwd: string;
  private nativeApiVersion: string;
  private listeners = new Set<AgentConnectionEventListener>();
  private beforeInvalidate = new Set<AgentConnectionBeforeSessionInvalidateListener>();
  private sessionId: string | null = null;
  private sessionTransitionGeneration = 0;
  private activeSessionId: string | undefined;
  private expectedTurnId: string | null = null;
  private subscriptionId: string | null = null;
  private subscriptionCursors: Array<{ streamId: string; seq: number }> = [];
  /** >0 while applying subscription/create replay (not live events). */
  private subscriptionReplayDepth = 0;
  private messages: unknown[] = [];
  private sessionHistoryRevision = 0;
  private pendingInitialMessages?: { sessionId: string; revision: number; promise: Promise<never[]> };
  private streamState: StreamState = createStreamState();
  private model: AgentConnectionModel | undefined;
  private thinkingLevel: ThinkingLevel = "medium";
  /** Empty until catalog enrichment; do not invent a full effort ladder. */
  private availableThinkingLevels: ThinkingLevel[] = [];
  private isStreaming = false;
  private isCompacting = false;
  /** Set when a projected compaction_end carries a skip/too-short warning. */
  private lastCompactionSkipMessage: string | null = null;
  private isBashRunning = false;
  /** Live provider retry attempt from model/queryRetrying (0 when idle). */
  private retryAttempt = 0;
  private sessionName: string | undefined;
  private sessionVersion = 0;
  private goal = emptyGoalState();
  private sessionActions = emptySessionActions();
  private queue: AgentConnectionQueueState = { steering: [], followUp: [] };
  /** Native queue entries (follow-up lane); steering previews stay local until drained. */
  private queueEntries: QueueWireEntry[] = [];
  private steeringMode: "all" | "one-at-a-time" = "all";
  private followUpMode: "all" | "one-at-a-time" = "all";
  private autoCompactionEnabled = true;
  private contextUsage: ReturnType<typeof occupancyToContextUsage> = emptyContextUsage();
  private permissionProfile: "default" | "autoReview" | "fullAccess" = "default";
  /** Explicit per-session auto-refine fields from the live session snapshot. */
  private sessionAutoRefine: { enabled?: boolean; turnInterval?: number } = {};
  private activeBashItemId: string | null = null;
  /** Keep the task handle after bash_end clears the active UI state. */
  private lastStartedBashItemId: string | null = null;
  private watchedSessionSubs = new Map<string, string>();
  private sessionWatchers = new Map<
    string,
    Set<AgentConnectionEventListener>
  >();
  private pendingReverse = new Map<
    string,
    { method: string; params: unknown; resolve: (v: unknown) => void; reject: (e: Error) => void }
  >();
  /**
   * Reverse-RPC emits deferred because they arrived while no listener was
   * attached. The server reissues pending approvals during
   * switchSession→createSubscription at boot, before the interactive mode
   * subscribes; emitting then would lose the request forever (nothing could
   * ever answer it). Flushed when the first listener registers.
   */
  private deferredReverseEmits: Array<() => void> = [];
  private readonly modelCatalogRefresh: ModelCatalogRefresh;
  private disposed = false;
  private trafficLog: NativeTrafficLog | undefined;
  private idleWaiters = new Set<() => void>();
  private rosterListeners = new Set<() => void>();
  /** Optimistic Esc/Ctrl+C already painted abort chrome; skip duplicate on turn/completed. */
  private localAbortChromePending = false;
  /** True when this turn emitted a file-mutating tool (edit / FileChange). */
  private turnSawFileMutation = false;

  private constructor(options: ConnectOptions) {
    this.trafficLog = options.trafficLog;
    this.rpc = new StdioJsonRpc(
      (line) => {
        this.trafficLog?.record(classifyNativeTrafficLine("tui-to-server", line));
        options.writeLine(line);
      },
      (line) => {
        this.trafficLog?.record(classifyNativeTrafficLine("server-to-tui", line));
      },
    );
    this.modelCatalogRefresh = new ModelCatalogRefresh(this.rpc);
    this.cwd = options.cwd ?? process.cwd();
    this.nativeApiVersion = options.nativeApiVersion ?? DEFAULT_NATIVE_API_VERSION;

    this.rpc.onNotification((method, params) => this.handleNotification(method, params));
    this.rpc.onServerRequest((id, method, params) => {
      void this.handleReverseRpc(id, method, params);
    });
  }

  static create(options: ConnectOptions): NativeAgentConnection {
    return new NativeAgentConnection(options);
  }

  static async connect(options: ConnectOptions & { clientInfo?: { name: string; version: string } }) {
    const conn = new NativeAgentConnection(options);
    await conn.initialize(options.clientInfo ?? { name: "devo-tui", version: "0.1.0" });
    await conn.newSession();
    return conn;
  }

  pushChunk(chunk: string): void {
    this.rpc.pushChunk(chunk);
  }

  private emit(event: AgentConnectionEvent): void {
    for (const listener of this.listeners) {
      try {
        const out = listener(event);
        if (out && typeof (out as Promise<void>).then === "function") {
          (out as Promise<void>).catch(() => {});
        }
      } catch {
        // ignore
      }
    }
  }

  private emitSessionEvent(event: Record<string, unknown>): void {
    this.emit({ type: "session_event", event: event as never });
  }

  /** After workspace/changes/updated, re-read summary and feed the workspace-level recap. */
  private async refreshWorkspaceFileChangesRecap(ev: {
    turnId?: unknown;
    scope?: unknown;
  }): Promise<void> {
    if (!this.sessionId) return;
    const turnId = ev.turnId != null ? String(ev.turnId) : undefined;
    const scope = String(ev.scope ?? "turn").toLowerCase();
    // Never silently widen turn-scoped requests to uncommitted — that floods
    // the footer in large dirty checkouts when turnId is missing.
    if (scope !== "uncommitted" && !turnId) {
      this.emitSessionEvent({
        type: "workspace_file_changes",
        changes: [],
        replace: true,
      });
      return;
    }
    const scopes = scope === "uncommitted" ? ["uncommitted"] : ["turn"];
    try {
      const result = await this.rpc.request("workspace/changes/read", {
        sessionId: this.sessionId,
        scopes,
        ...(scopes[0] === "turn" && turnId ? { turnId } : {}),
        diffDetail: "summary",
      });
      const changes = fileChangeSummariesFromWorkspaceViews(result);
      // Guard: a broken turn baseline can look like the entire dirty worktree.
      const FLOOD_GUARD = 80;
      this.emitSessionEvent({
        type: "workspace_file_changes",
        changes: changes.length > FLOOD_GUARD ? [] : changes,
        replace: true,
      });
    } catch {
      // Best-effort: per-tool inline diffs still work without the workspace recap.
    }
  }

  /** Seed workspace-level recap after resume / initial snapshot.
   *
   * Prefer staying quiet on resume: uncommitted scope can be thousands of files
   * in a dirty monorepo and dominates the footer. Turn completions own the recap.
   */
  private async seedWorkspaceFileChangesRecap(): Promise<void> {
    // Clear any stale workspace recap from a previous session rather than
    // re-scanning the whole worktree.
    this.emitSessionEvent({
      type: "workspace_file_changes",
      changes: [],
      replace: true,
    });
  }

  private handleNotification(method: string, params: unknown): void {
    void this.handleNotificationAsync(method, params);
  }

  private async handleNotificationAsync(method: string, params: unknown): Promise<void> {
    const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;
    const notificationSessionId = String(
      p.sessionId ??
        p.session_id ??
        (p.session as { id?: unknown } | undefined)?.id ??
        (p.turn as { sessionId?: unknown } | undefined)?.sessionId ??
        "",
    );
    const appliesToActiveSession = !notificationSessionId || notificationSessionId === this.sessionId;
    // A delayed notification must not project an old session into the active UI; explicit watchers still receive it.
    if (notificationSessionId && !appliesToActiveSession) {
      this.fanoutWatchedSession(method, params);
      return;
    }
    if (
      appliesToActiveSession &&
      (method.startsWith("turn/") ||
        method.startsWith("item/") ||
        method.startsWith("message/") ||
        method.startsWith("assistantMessage/") ||
        method.startsWith("tool/") ||
        method.startsWith("agent/") ||
        method.startsWith("session/item") ||
        method.startsWith("session/branch/"))
    ) {
      this.sessionHistoryRevision += 1;
    }

    if (method === "turn/started" || method === "turn/resumed") {
      // Historical turn/started without a live registry owner must not
      // resurrect Waiting; busy is seeded from the subscription snapshot.
      if (this.subscriptionReplayDepth === 0) {
        this.isStreaming = true;
        this.retryAttempt = 0;
        this.localAbortChromePending = false;
        this.turnSawFileMutation = false;
        const turn = p.turn as { id?: string } | undefined;
        if (turn?.id) this.expectedTurnId = String(turn.id);
      }
      if (this.subscriptionReplayDepth === 0) {
        // Drain notifications can race the UI; re-list so Follow-up/Steering banners clear.
        void this.refreshQueueFromServer();
      }
    }
    if (method === "turn/completed") {
      this.isStreaming = false;
      this.retryAttempt = 0;
      this.expectedTurnId = null;
      const turn = (p.turn ?? p) as { kind?: string; status?: string; id?: string };
      if (String(turn.kind ?? "").toLowerCase() === "compaction") {
        this.isCompacting = false;
      }
      this.resolveIdleWaiters();
      if (this.subscriptionReplayDepth === 0) {
        void this.refreshQueueFromServer();
        const status = String(turn.status ?? "").toLowerCase();
        // Interrupted / no file mutations: clear footer. Never re-scan a dirty
        // monorepo worktree into the recap (D03).
        if (status === "interrupted" || !this.turnSawFileMutation) {
          void this.seedWorkspaceFileChangesRecap();
        } else {
          const completedTurnId = turn.id != null ? String(turn.id) : undefined;
          if (completedTurnId) {
            void this.refreshWorkspaceFileChangesRecap({
              turnId: completedTurnId,
              scope: "turn",
            });
          } else {
            void this.seedWorkspaceFileChangesRecap();
          }
        }
      }
      this.turnSawFileMutation = false;
    }

    if (method === "session/statusChanged") {
      const flags = Array.isArray(p.flags) ? p.flags.map(String) : [];
      this.isCompacting = flags.includes("compacting");
      // Rising edge only — turn/completed (and abort()) own the falling edge.
      // Clearing here desyncs InteractiveMode (agent_start) from Native isStreaming
      // and makes Enter call turn/start while the server turn is still live.
      // Skip Active promotion during subscription replay (abandoned InProgress).
      if (
        this.subscriptionReplayDepth === 0 &&
        (String(p.status ?? "").toLowerCase() === "active" || Boolean(p.activeTurnId))
      ) {
        this.isStreaming = true;
        if (p.activeTurnId) this.expectedTurnId = String(p.activeTurnId);
      }
      this.notifyRoster();
    }

    if (method === "queue/updated" && this.subscriptionReplayDepth === 0) {
      void this.refreshQueueFromServer();
    }

    if (method === "session/metadataUpdated") {
      const session = (p.session ?? p) as Record<string, unknown>;
      this.applySessionSnapshot(session);
      this.notifyRoster();
    }

    if (
      method === "session/created" ||
      method === "session/deleted" ||
      method === "session/archived" ||
      method === "session/closed" ||
      method === "agent/started" ||
      method === "agent/progress" ||
      method === "agent/completed" ||
      method === "credential/changed" ||
      method === "session/schedule/changed"
    ) {
      this.notifyRoster();
    }

    const turnCompletedInterrupted =
      method === "turn/completed" &&
      String(((p.turn ?? p) as { status?: string }).status ?? "").toLowerCase() === "interrupted";
    const turnStatusInterrupted =
      method === "turn/statusChanged" && String(p.status ?? "").toLowerCase() === "interrupted";
    const skipDuplicateAbortChrome =
      this.localAbortChromePending && (turnCompletedInterrupted || turnStatusInterrupted);
    // Only clear on turn/completed — statusChanged can precede it and would
    // otherwise re-enable a second "Operation aborted" line.
    if (skipDuplicateAbortChrome && turnCompletedInterrupted) {
      this.localAbortChromePending = false;
    }

    // A decided approval item means the permission request was answered —
    // possibly from ANOTHER client. Open dialogs for the same approval must
    // not linger as zombies (first valid answer wins server-side).
    const resolvedApprovalId = approvalResolutionFromNotification(method, params);
    if (resolvedApprovalId) {
      this.emit({ type: "approval_resolved", approvalId: resolvedApprovalId });
    }
    // Same for pending user-input questions: the server broadcasts
    // serverRequest/resolved when any controller answered first.
    if (method === "serverRequest/resolved") {
      const resolvedRequestId = p.requestId ?? p.request_id;
      if (typeof resolvedRequestId === "string" && resolvedRequestId) {
        this.emit({ type: "server_request_resolved", requestId: resolvedRequestId });
      }
    }

    for (const ev of nativeNotificationToPiEvents(method, params, this.streamState)) {
      if (ev.type === "compaction_start") {
        this.isCompacting = true;
        this.lastCompactionSkipMessage = null;
      }
      if (ev.type === "compaction_end") {
        // Clear before emit so waitForCompactionIdle unblocks; InteractiveMode
        // rebuildChatFromMessages calls getMessages() itself.
        this.isCompacting = false;
        const end = ev as {
          errorMessage?: string;
          errorSeverity?: string;
          result?: unknown;
        };
        if (
          !end.result &&
          end.errorSeverity === "warning" &&
          typeof end.errorMessage === "string" &&
          end.errorMessage.length > 0
        ) {
          this.lastCompactionSkipMessage = end.errorMessage;
        }
        this.resolveIdleWaiters();
      }
      if (ev.type === "workspace_changes_updated") {
        // Ignore workspace wake-ups during local abort, or when this turn has
        // not mutated files (ghost-baseline noise floods dirty checkouts).
        if (
          !this.localAbortChromePending &&
          !skipDuplicateAbortChrome &&
          this.turnSawFileMutation
        ) {
          void this.refreshWorkspaceFileChangesRecap(ev as {
            turnId?: unknown;
            scope?: unknown;
          });
        }
        continue;
      }
      if (
        skipDuplicateAbortChrome &&
        (ev.type === "message_end" || ev.type === "agent_end" || ev.type === "tool_execution_end")
      ) {
        continue;
      }
      if (ev.type === "tool_execution_end") {
        const toolName = String(ev.toolName ?? "").toLowerCase();
        if (toolName === "edit" || toolName === "apply_patch" || toolName === "write") {
          this.turnSawFileMutation = true;
        }
        const details = (ev.result as { details?: { diffs?: unknown[] } } | undefined)?.details;
        if (Array.isArray(details?.diffs) && details.diffs.length > 0) {
          this.turnSawFileMutation = true;
        }
      }
      if (ev.type === "tool_pending" || ev.type === "tool_execution_start") {
        const toolName = String(ev.toolName ?? "").toLowerCase();
        if (toolName === "edit" || toolName === "apply_patch" || toolName === "write") {
          this.turnSawFileMutation = true;
        }
      }
      if (this.subscriptionReplayDepth > 0 && ev.type === "agent_start") {
        continue;
      }
      this.emitSessionEvent(ev);
      if (ev.type === "auto_retry_start") {
        this.retryAttempt = Number((ev as { attempt?: unknown }).attempt ?? 0);
      }
      if (ev.type === "auto_retry_end") {
        this.retryAttempt = 0;
      }
      if (ev.type === "bash_start") this.isBashRunning = true;
      if (ev.type === "bash_end") {
        this.isBashRunning = false;
        this.activeBashItemId = null;
        this.streamState.userBashUiMounted = false;
      }
      if (ev.type === "goal_update") {
        this.goal = (ev.goal ?? emptyGoalState()) as never;
      }
      if (ev.type === "session_action_update") {
        const actions = (ev.actions ?? {}) as {
          steering?: string[];
          followUps?: string[];
          queuedCount?: number;
        };
        this.queue = {
          steering: [...(actions.steering ?? [])],
          followUp: [...(actions.followUps ?? [])],
        };
        this.syncActionsFromQueue();
      }
      if (ev.type === "context_usage_update") {
        this.contextUsage = ev.contextUsage as never;
      }
      if (ev.type === "session_info_changed" && ev.name != null) {
        this.sessionName = String(ev.name);
      }
    }

    this.fanoutWatchedSession(method, params);
  }

  private fanoutWatchedSession(method: string, params: unknown): void {
    if (this.sessionWatchers.size === 0) return;
    const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;
    const sid = String(
      p.sessionId ??
        p.session_id ??
        (p.session as { id?: string } | undefined)?.id ??
        (p.turn as { sessionId?: string } | undefined)?.sessionId ??
        "",
    );
    for (const [watchedId, listeners] of this.sessionWatchers) {
      if (sid && sid !== watchedId) continue;
      if (!sid && watchedId !== this.sessionId) continue;
      for (const ev of nativeNotificationToPiEvents(method, params, createStreamState())) {
        for (const listener of listeners) {
          try {
            void listener({ type: "session_event", event: ev as never });
          } catch {
            // ignore
          }
        }
      }
    }
  }

  private applySessionSnapshot(session: Record<string, unknown>): void {
    if (session.id) {
      this.sessionId = String(session.id);
      this.activeSessionId = this.sessionId;
    }
    if (typeof session.version === "number") this.sessionVersion = session.version;
    if (session.title || session.name) this.sessionName = String(session.title ?? session.name);
    if (session.cwd || session.directory) this.cwd = String(session.cwd ?? session.directory);
    if (session.model) {
      this.model = normalizeAgentModel(session.model as Record<string, unknown>) ?? this.model;
      this.applyAvailableThinkingLevelsFromModel(this.model);
    }
    const settings = session.settings as Record<string, unknown> | undefined;
    if (settings?.reasoningEffort || settings?.reasoning_effort) {
      const raw = String(settings.reasoningEffort ?? settings.reasoning_effort);
      this.thinkingLevel = normalizeThinkingSelection(
        raw,
        this.availableThinkingLevels,
        thinkingLevelMapFromModel(this.model),
      );
    }
    if (settings?.permissionProfile || settings?.permission_profile) {
      const profile = String(settings.permissionProfile ?? settings.permission_profile);
      if (profile === "autoReview" || profile === "fullAccess" || profile === "default") {
        this.permissionProfile = profile;
      }
    }
    const autoRefineEnabled = settings?.autoRefineEnabled ?? settings?.auto_refine_enabled;
    if (typeof autoRefineEnabled === "boolean") {
      this.sessionAutoRefine.enabled = autoRefineEnabled;
    }
    const autoRefineInterval = settings?.autoRefineTurnInterval ?? settings?.auto_refine_turn_interval;
    if (typeof autoRefineInterval === "number" && Number.isFinite(autoRefineInterval)) {
      this.sessionAutoRefine.turnInterval = autoRefineInterval;
    }
  }

  /**
   * Fill unset per-session auto-refine fields from the user's settings.json
   * preference. Without this the user-level `autoRefine` block was dead
   * config: the server only honors the per-session fields and runs its own
   * defaults (enabled, every 25 turns). Only fills holes — an explicit
   * session-level value always wins over the user default.
   */
  async applyAutoRefineDefaults(override: { enabled?: boolean; turnInterval?: number }): Promise<void> {
    // Fetch the live snapshot first: the session may not have been applied to
    // this instance yet, and explicit session-level values must suppress the
    // user-default fill.
    await this.refreshSessionSnapshot();
    const patch: Record<string, unknown> = {};
    if (override.enabled !== undefined && this.sessionAutoRefine.enabled === undefined) {
      patch.autoRefineEnabled = override.enabled;
    }
    if (override.turnInterval !== undefined && this.sessionAutoRefine.turnInterval === undefined) {
      patch.autoRefineTurnInterval = override.turnInterval;
    }
    if (Object.keys(patch).length === 0) return;
    // SessionSettingsPatch fields ride nested under `settings` — top-level
    // unknown keys are silently ignored by the server.
    await this.updateSessionMetadata({ settings: patch });
  }

  /** Merge catalog ModelInfo (reasoning, levels, window) into the live session model. */
  private async enrichModelFromCatalog(sessionId: string | null = this.sessionId): Promise<void> {
    const model = this.model;
    if (!sessionId || !model) return;
    try {
      const catalog = await this.getAvailableModels();
      if (this.sessionId !== sessionId || this.model !== model) return;
      const match = findCatalogModel(catalog, model);
      if (!match) {
        this.ensureDefaultContextUsage();
        return;
      }
      this.model = {
        ...match,
        id: model.id || match.id,
        provider: preferVendorProvider(model.provider, match.provider),
      };
      this.applyAvailableThinkingLevelsFromModel(match);
      this.ensureDefaultContextUsage();
    } catch {
      if (this.sessionId === sessionId) this.ensureDefaultContextUsage();
    }
  }

  private applyAvailableThinkingLevelsFromModel(model: AgentConnectionModel | null | undefined): void {
    const levels = thinkingLevelsFromModel(model);
    if (levels.length === 0) return;
    this.availableThinkingLevels = levels;
    if (!levels.includes(this.thinkingLevel)) {
      this.thinkingLevel = pickDefaultThinkingLevel(levels);
    }
  }

  /** Keep footer % window aligned with the active model (switch must update). */
  private syncContextUsageWindowFromModel(): void {
    const window = Number(this.model?.contextWindow ?? 0);
    if (window <= 0) return;
    const prev = this.contextUsage;
    const tokens = Number(prev?.tokens ?? 0);
    if (prev && Number(prev.contextWindow ?? 0) === window) return;
    const percent =
      window > 0 ? Math.min(100, Math.round((tokens / window) * 1000) / 10) : 0;
    this.contextUsage = { tokens, contextWindow: window, percent } as never;
  }

  /** Footer shows `0 (0%)` once a model window is known. */
  private ensureDefaultContextUsage(): void {
    if (this.contextUsage) {
      this.syncContextUsageWindowFromModel();
      return;
    }
    const window = Number(this.model?.contextWindow ?? 0);
    if (window <= 0) return;
    this.contextUsage = { tokens: 0, contextWindow: window, percent: 0 } as never;
  }

  private notifyRoster(): void {
    for (const listener of this.rosterListeners) {
      try {
        listener();
      } catch {
        // ignore
      }
    }
  }

  private async handleReverseRpc(id: JsonRpcId, method: string, params: unknown): Promise<void> {
    const requestId = String(id);
    this.pendingReverse.set(requestId, {
      method,
      params,
      resolve: () => {},
      reject: () => {},
    });
    const emitRequest = () => {
      const ui = reverseRpcToExtensionUiRequest(requestId, method, params);
      this.emit({ type: "extension_ui_request", request: ui as never });
    };
    if (this.listeners.size === 0) {
      this.deferredReverseEmits.push(emitRequest);
      return;
    }
    emitRequest();
  }

  private resolveIdleWaiters(): void {
    for (const w of this.idleWaiters) w();
    this.idleWaiters.clear();
  }

  private requireSessionId(): string {
    if (!this.sessionId) throw new Error("NativeAgentConnection: no session bound");
    return this.sessionId;
  }

  private buildState(): AgentConnectionState {
    return {
      activeSessionId: this.activeSessionId ?? this.sessionId ?? undefined,
      cwd: this.cwd,
      model: this.model,
      thinkingLevel: this.thinkingLevel,
      serviceTier: "default",
      availableThinkingLevels: this.availableThinkingLevels,
      isStreaming: this.isStreaming,
      isCompacting: this.isCompacting,
      isBashRunning: this.isBashRunning,
      retryAttempt: this.retryAttempt,
      steeringMode: this.steeringMode,
      followUpMode: this.followUpMode,
      sessionId: this.sessionId ?? "",
      sessionName: this.sessionName,
      sessionFile: this.sessionRolloutPath(),
      leafId: null,
      autoCompactionEnabled: this.autoCompactionEnabled,
      messageCount: this.messages.length,
      sessionActions: {
        ...this.sessionActions,
        queuedCount: this.queue.steering.length + this.queue.followUp.length,
        steering: [...this.queue.steering],
        followUps: [...this.queue.followUp],
      },
      compactionCount: 0,
      goal: this.goal as never,
      scopedModels: [],
      activeToolNames: ["ipython"],
      contextUsage: this.contextUsage as never,
    };
  }

  /** Durable session rollout JSONL path (`~/.devo/sessions/<id>.jsonl`). */
  private sessionRolloutPath(): string | undefined {
    if (!this.sessionId) return undefined;
    return path.join(resolveDevoHome(), "sessions", `${this.sessionId}.jsonl`);
  }

  async initialize(clientInfo: { name: string; version: string }): Promise<unknown> {
    return this.rpc.request("initialize", {
      protocolVersion: 1,
      _meta: {
        devo: {
          protocol: "native",
          typedItems: true,
          apiVersion: this.nativeApiVersion,
        },
      },
      clientCapabilities: {
        fs: { readTextFile: false, writeTextFile: false },
        terminal: false,
      },
      clientInfo: {
        name: clientInfo.name,
        title: "Devo",
        version: clientInfo.version,
      },
    });
  }

  private async createSubscription(
    options: {
      supplementalRefreshMode?: "blocking" | "deferred";
      transitionGeneration?: number;
    } = {},
  ): Promise<void> {
    const sessionId = this.requireSessionId();
    const transitionGeneration = options.transitionGeneration ?? this.sessionTransitionGeneration;
    const previousSubscriptionId = this.subscriptionId;
    if (previousSubscriptionId) {
      try {
        await this.rpc.request("subscription/unsubscribe", {
          subscriptionId: previousSubscriptionId,
        });
      } catch {
        // ignore
      }
      if (this.subscriptionId === previousSubscriptionId) this.subscriptionId = null;
      if (transitionGeneration !== this.sessionTransitionGeneration) return;
    }
    const after = this.subscriptionCursors.map((c) => ({
      streamId: c.streamId,
      seq: c.seq,
    }));
    const result = (await this.rpc.request("subscription/create", {
      selectors: [{ kind: "session", sessionId }],
      includeSnapshot: true,
      after,
    })) as {
      subscriptionId?: string;
      snapshots?: Array<Record<string, unknown>>;
      replay?: Array<Record<string, unknown>>;
      cursors?: Array<{ streamId: string; seq: number }>;
      pendingControlRequests?: Array<Record<string, unknown>>;
    };
    if (transitionGeneration !== this.sessionTransitionGeneration) {
      const staleSubscriptionId = result?.subscriptionId;
      if (staleSubscriptionId) {
        void this.rpc
          .request("subscription/unsubscribe", { subscriptionId: staleSubscriptionId })
          .catch(() => {});
      }
      return;
    }
    this.subscriptionId = result?.subscriptionId ?? null;
    this.subscriptionCursors = result?.cursors ?? [];

    let snapshotHasActiveTurn = false;
    for (const snapshot of result?.snapshots ?? []) {
      const data = (snapshot.data ?? snapshot) as Record<string, unknown>;
      const session = (data.session ?? data) as Record<string, unknown>;
      if (session && typeof session === "object") this.applySessionSnapshot(session);
      const activeTurn = (data.activeTurn ?? data.active_turn) as
        | { id?: string }
        | null
        | undefined;
      if (activeTurn && typeof activeTurn === "object" && activeTurn.id) {
        snapshotHasActiveTurn = true;
        this.isStreaming = true;
        this.expectedTurnId = String(activeTurn.id);
      }
      const items = (data.items ?? data.recentItems) as unknown[] | undefined;
      if (Array.isArray(items)) {
        this.messages = nativeItemsToAgentMessages(items);
      }
    }

    this.subscriptionReplayDepth += 1;
    try {
      for (const envelope of result?.replay ?? []) {
        const notification = (envelope.notification ?? envelope) as Record<string, unknown>;
        const method = String(notification.method ?? envelope.method ?? "");
        const params = notification.params ?? envelope.params;
        if (method) this.handleNotification(method, params);
      }
    } finally {
      this.subscriptionReplayDepth -= 1;
    }

    if (!snapshotHasActiveTurn) {
      this.isStreaming = false;
      this.expectedTurnId = null;
    }

    for (const pending of result?.pendingControlRequests ?? []) {
      const id = (pending.id ?? pending.requestId) as JsonRpcId;
      const method = String(pending.method ?? "");
      if (id != null && method) {
        void this.handleReverseRpc(id, method, pending.params);
      }
    }

    if (this.subscriptionId && this.subscriptionCursors.length > 0) {
      try {
        await this.rpc.request("subscription/ack", {
          subscriptionId: this.subscriptionId,
          cursors: this.subscriptionCursors,
        });
      } catch {
        // best-effort
      }
    }

    if (transitionGeneration !== this.sessionTransitionGeneration) return;
    if (options.supplementalRefreshMode === "deferred") return;
    await this.refreshContextUsage(sessionId);
    await this.enrichModelFromCatalog(sessionId);
    await this.refreshQueueFromServer(sessionId).catch(() => {});
  }

  private async refreshContextUsage(sessionId: string | null = this.sessionId): Promise<void> {
    if (!sessionId) return;
    try {
      const result = (await this.rpc.request("context/usage/read", { sessionId })) as { occupancy?: unknown };
      if (this.sessionId !== sessionId) return;
      const next = occupancyToContextUsage(result?.occupancy);
      if (next) {
        this.contextUsage = next as never;
        // Occupancy snapshots can still carry the previous model's window after
        // a mid-session model switch; prefer the active catalog window.
        this.syncContextUsageWindowFromModel();
        this.emitSessionEvent({ type: "context_usage_update", contextUsage: this.contextUsage });
      } else {
        this.ensureDefaultContextUsage();
      }
    } catch {
      if (this.sessionId === sessionId) this.ensureDefaultContextUsage();
    }
  }


  private async refreshSessionSnapshot(): Promise<void> {
    if (!this.sessionId) return;
    try {
      const result = (await this.rpc.request("session/read", {
        sessionId: this.sessionId,
      })) as { session?: Record<string, unknown> };
      if (result?.session) this.applySessionSnapshot(result.session);
    } catch {
      // best-effort
    }
  }

  private async refreshPermissionProfile(): Promise<void> {
    await this.refreshSessionSnapshot();
  }

  private async updateSessionMetadata(patch: Record<string, unknown>): Promise<Record<string, unknown> | undefined> {
    const sessionId = this.requireSessionId();
    await this.refreshSessionSnapshot();
    const result = (await this.rpc.request("session/metadata/update", {
      sessionId,
      expectedVersion: this.sessionVersion || 0,
      ...patch,
    })) as { session?: Record<string, unknown> };
    if (result?.session) this.applySessionSnapshot(result.session);
    return result?.session;
  }

  subscribe(listener: AgentConnectionEventListener): () => void {
    this.listeners.add(listener);
    this.flushDeferredReverseEmits();
    return () => this.listeners.delete(listener);
  }

  private flushDeferredReverseEmits(): void {
    if (this.deferredReverseEmits.length === 0) return;
    const deferred = this.deferredReverseEmits;
    this.deferredReverseEmits = [];
    for (const emit of deferred) emit();
  }

  onBeforeSessionInvalidate(listener: AgentConnectionBeforeSessionInvalidateListener): () => void {
    this.beforeInvalidate.add(listener);
    return () => this.beforeInvalidate.delete(listener);
  }

  private fireBeforeInvalidate(): void {
    for (const l of this.beforeInvalidate) {
      try {
        l();
      } catch {
        // ignore
      }
    }
  }

  async getState(): Promise<AgentConnectionState> {
    return this.buildState();
  }

  async getInitialSnapshot(): Promise<AgentConnectionSnapshot> {
    const sessionId = this.sessionId;
    const pending = this.pendingInitialMessages;
    let messagesPromise: Promise<never[]>;
    if (sessionId && pending?.sessionId === sessionId) {
      messagesPromise = pending.promise.then((messages) => {
        if (pending.revision === this.sessionHistoryRevision && pending.sessionId === this.sessionId) {
          return messages;
        }
        const currentPending = this.pendingInitialMessages;
        if (currentPending && currentPending !== pending && currentPending.sessionId === this.sessionId) {
          return currentPending.promise;
        }
        const currentSessionId = this.sessionId;
        if (!currentSessionId) return [];
        const revision = this.sessionHistoryRevision;
        const promise = this.getMessages();
        this.pendingInitialMessages = { sessionId: currentSessionId, revision, promise };
        return promise;
      });
    } else {
      messagesPromise = this.getMessages();
    }
    const [messages, children] = await Promise.all([
      measureAsync("snapshot.history", () => messagesPromise),
      measureAsync("snapshot.rlm_child_snapshots", () => this.getRlmChildSnapshots()),
    ]);
    if (this.sessionId !== sessionId) return this.getInitialSnapshot();
    void this.seedWorkspaceFileChangesRecap();
    return {
      state: this.buildState(),
      messages: messages as never,
      children: children as never,
      replay: { status: "complete", toSequence: 0 },
    };
  }
  async getMessages(): Promise<never[]> {
    const sessionId = this.sessionId;
    if (!sessionId) return [...this.messages] as never[];
    const historyRevision = this.sessionHistoryRevision;
    let loadedMessages: unknown[] | undefined;
    try {
      const items: unknown[] = [];
      let cursor: string | undefined;
      do {
        const page = (await this.rpc.request("session/items/list", {
          sessionId,
          ...(cursor ? { cursor } : {}),
          limit: 500,
        })) as { data?: unknown[]; nextCursor?: string | null };
        if (Array.isArray(page?.data)) items.push(...page.data);
        cursor = page?.nextCursor ?? undefined;
      } while (cursor);
      let messages = nativeItemsToAgentMessages(items);
      // Warning items may be missing from items/list; turn.error is durable on
      // session/turns/list and restores the live failure chrome after resume.
      try {
        const turns: unknown[] = [];
        let turnCursor: string | undefined;
        do {
          const page = (await this.rpc.request("session/turns/list", {
            sessionId,
            ...(turnCursor ? { cursor: turnCursor } : {}),
            limit: 200,
          })) as { data?: unknown[]; nextCursor?: string | null };
          if (Array.isArray(page?.data)) turns.push(...page.data);
          turnCursor = page?.nextCursor ?? undefined;
        } while (turnCursor);
        messages = applyFailedTurnsToAgentMessages(messages, turns);
      } catch {
        // best-effort — items alone still restore chat text
      }
      loadedMessages = messages;
      if (this.sessionId === sessionId && this.sessionHistoryRevision === historyRevision) {
        this.messages = messages;
      }
    } catch {
      // keep cache
    }
    if (this.sessionId !== sessionId) return [...this.messages] as never[];
    return [...(loadedMessages ?? this.messages)] as never[];
  }

  async getRlmChildSnapshots() {
    if (!this.sessionId) return [];
    try {
      const result = (await this.rpc.request("agent/list", {
        sessionId: this.sessionId,
      })) as { agents?: unknown[] };
      const agents = Array.isArray(result?.agents) ? result.agents : [];
      const out: Array<{
        id: string;
        parentId?: string;
        activeSessionId?: string;
        label: string;
        status: "queued" | "running" | "done" | "error" | "cancelled";
        sessionDir: string;
        recap?: string;
      }> = [];
      for (const entry of agents) {
        if (!entry || typeof entry !== "object") continue;
        const envelope = entry as Record<string, unknown>;
        const item = (envelope.item ?? envelope) as Record<string, unknown>;
        const type = String(item.type ?? "").toLowerCase();
        if (type !== "subagent" && type !== "sub_agent") continue;
        const agentSessionId = String(item.agentSessionId ?? item.agent_session_id ?? "");
        const itemId = String(envelope.id ?? item.id ?? agentSessionId);
        if (!itemId && !agentSessionId) continue;
        // Native subAgent items carry the lifecycle in `state` (completed /
        // interrupted / cancelled); fall back through `status` for older
        // shapes. A miss must not default a terminal agent to "running".
        const statusRaw = String(
          item.status ?? item.state ?? envelope.status ?? envelope.state ?? "idle",
        ).toLowerCase();
        const status =
          statusRaw === "queued"
            ? "queued"
            : statusRaw === "done" || statusRaw === "completed"
              ? "done"
              : statusRaw === "error" || statusRaw === "failed"
                ? "error"
                : statusRaw === "cancelled" ||
                    statusRaw === "canceled" ||
                    statusRaw === "interrupted"
                  ? "cancelled"
                  : "running";
        out.push({
          id: itemId || agentSessionId,
          parentId: undefined,
          activeSessionId: agentSessionId || undefined,
          label: String(item.role ?? item.task ?? itemId) || "agent",
          status,
          sessionDir: this.cwd,
          recap: typeof item.task === "string" ? item.task : undefined,
        });
      }
      return out as never;
    } catch {
      return [];
    }
  }

  async getSessionHeader() {
    if (!this.sessionId) return undefined;
    return {
      type: "session" as const,
      id: this.sessionId,
      timestamp: new Date().toISOString(),
      cwd: this.cwd,
      name: this.sessionName,
    };
  }

  async getCommands(): Promise<AgentConnectionSlashCommand[]> {
    const extras = DEVO_SLASH_COMMANDS.filter((c) => !STRIP_VENDOR_COMMANDS.has(`/${c.name}`));
    // InteractiveMode lists bundled skills as `/skill:<name>` (source: "skill") so they
    // appear in the slash autocomplete like InteractiveMode on the daemon path.
    let skillCommands: AgentConnectionSlashCommand[] = [];
    try {
      const skillsResult = (await this.rpc.request("skill/list", {
        cwd: this.cwd,
        forceReload: false,
      })) as { skills?: Array<Record<string, unknown>> };
      skillCommands = (skillsResult?.skills ?? [])
        .filter((s) => s.enabled !== false)
        .map((s) => {
          const name = String(s.name ?? s.id ?? "").trim();
          return {
            name: `skill:${name}`,
            description: String(s.description ?? s.short_description ?? name),
            source: "skill" as const,
            sourceInfo: (() => {
              const info = mapSkillSourceInfo(s);
              return { ...info, scope: info.scope === "path" ? "temporary" as const : info.scope };
            })(),
          };
        })
        .filter((c) => c.name !== "skill:");
    } catch {
      // keep empty — autocomplete still works for builtins/extras
    }
    return [...extras, ...skillCommands];
  }

  async getResourceSnapshot(): Promise<AgentConnectionResourceSnapshot> {
    const snap = emptyResourceSnapshot();
    try {
      await measureAsync("resources.skill_list", async () => {
        const skillsResult = (await this.rpc.request("skill/list", {
          cwd: this.cwd,
          forceReload: false,
        })) as { skills?: Array<Record<string, unknown>> };
        snap.skills = mapSkillsToResourceSkills(
          (skillsResult?.skills ?? []) as never,
        ) as never;
      });
    } catch {
      // keep empty
    }
    // tool/list is in the Native method catalog but not routed on the server yet.
    try {
      await measureAsync("resources.mcp_list", async () => {
        const mcpResult = (await this.rpc.request("mcp/list", {})) as {
          servers?: Array<Record<string, unknown>>;
        };
        snap.extensions = mapMcpToExtensions((mcpResult?.servers ?? []) as never) as never;
      });
    } catch {
      // keep empty
    }
    try {
      await measureAsync("resources.workspace_changes_read", async () => {
        if (this.sessionId) {
          const changes = await this.rpc.request("workspace/changes/read", {
            sessionId: this.sessionId,
            scopes: ["uncommitted"],
            diffDetail: "summary",
          });
          snap.contextFiles = workspacePathsFromChanges(changes).map((path) => ({ path }));
        }
      });
    } catch {
      // optional
    }
    return snap;
  }

  async getModelCatalog(): Promise<AgentConnectionModelCatalog> {
    const { models, configuredProviders } = await this.loadModelCatalog();
    return { models, configuredProviders };
  }

  async refreshModelCatalog(): Promise<AgentConnectionModelCatalog> {
    await this.modelCatalogRefresh.refresh();
    return this.getModelCatalog();
  }

  async getAvailableModels(): Promise<AgentConnectionModel[]> {
    const { models } = await this.loadModelCatalog();
    return models;
  }

  /**
   * Load the Native model directory plus providers that already have usable
   * auth (user Connections and/or credentials in auth.json).
   *
   * InteractiveMode's `/model` UI keys "require sign in" off
   * `configuredProviders`. AuthStorage cannot read Devo's auth.json shape
   * (`credentials.<id>.{kind,value}`), so this must come from Native RPCs.
   */
  private async loadModelCatalog(): Promise<AgentConnectionModelCatalog> {
    const byKey = new Map<string, AgentConnectionModel>();
    const configured = new Set<string>();
    const add = (m: AgentConnectionModel | null) => {
      if (!m) return;
      byKey.set(`${m.provider}/${m.id}`, m);
    };
    const markConfigured = (providerId: string | undefined) => {
      const id = String(providerId ?? "").trim();
      if (id) configured.add(id);
    };
    const [modelListResponse, providerListResponse, credentialsResponse] = await Promise.all([
      measureAsync("model_catalog.model_list", () => this.rpc.request("model/list", {})).catch(() => undefined),
      measureAsync("model_catalog.provider_list", () => this.rpc.request("provider/list", {})).catch(() => undefined),
      measureAsync("model_catalog.credential_list", () => this.listCredentials()).catch(() => undefined),
    ]);
    try {
      const result = modelListResponse as {
        models?: Array<Record<string, unknown>>;
        providers?: Array<{ models?: Array<Record<string, unknown>> }>;
      } | undefined;
      if (Array.isArray(result?.models)) {
        for (const m of result.models) add(normalizeAgentModel(m));
      }
      if (Array.isArray(result?.providers)) {
        for (const p of result.providers) {
          if (Array.isArray(p.models)) {
            for (const m of p.models) add(normalizeAgentModel(m));
          }
        }
      }
    } catch {
      // fall through
    }
    try {
      const providers = providerListResponse as {
        providers?: Array<{
          id?: string;
          name?: string;
          models?: Array<Record<string, unknown>> | Record<string, Record<string, unknown>>;
        }>;
        connectedProviderIds?: string[];
        connectionModels?: Record<string, Record<string, Record<string, unknown>>>;
      } | undefined;
      for (const id of providers?.connectedProviderIds ?? []) {
        markConfigured(id);
      }
      for (const p of providers?.providers ?? []) {
        const providerId = String(p.id ?? p.name ?? "");
        const models = p.models;
        if (Array.isArray(models)) {
          for (const m of models) {
            add(normalizeAgentModel({ ...m, provider: m.provider ?? providerId }));
          }
        } else if (models && typeof models === "object") {
          for (const [modelId, info] of Object.entries(models)) {
            add(
              normalizeAgentModel({
                ...(info as Record<string, unknown>),
                id: modelId,
                provider: providerId,
              }),
            );
          }
        }
      }
      if (providers?.connectionModels) {
        for (const [providerId, models] of Object.entries(providers.connectionModels)) {
          markConfigured(providerId);
          for (const [modelId, info] of Object.entries(models)) {
            add(
              normalizeAgentModel({
                ...(info as Record<string, unknown>),
                id: modelId,
                provider: providerId,
              }),
            );
          }
        }
      }
    } catch {
      // optional enrichment
    }
    try {
      const listed = credentialsResponse as { credentials?: Array<{ provider?: string }> } | undefined;
      for (const credential of listed?.credentials ?? []) {
        markConfigured(credential.provider);
      }
    } catch {
      // optional enrichment
    }
    const models = [...byKey.values()];
    if (models.length === 0 && this.model) {
      return {
        models: [this.model],
        configuredProviders: [...configured],
      };
    }
    return {
      models,
      configuredProviders: [...configured],
    };
  }

  async getSessionStats() {
    await this.refreshContextUsage();
    const messages = (await this.getMessages()) as Array<{ role?: string; content?: unknown }>;
    const counts = countMessageStats(messages);
    const usage = this.contextUsage as { tokens?: number } | undefined;
    const tokensUsed = Number(usage?.tokens ?? 0);
    return {
      sessionFile: this.sessionRolloutPath(),
      sessionId: this.sessionId ?? "",
      ...counts,
      tokens: {
        input: tokensUsed,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        total: tokensUsed,
      },
      cost: 0,
      contextUsage: this.contextUsage,
    };
  }

  async getContextTree() {
    await this.refreshContextUsage();
    const zeroUsage = {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    };
    const tokensUsed = Number(
      (this.contextUsage as { tokens?: number } | undefined)?.tokens ?? 0,
    );
    const ownUsage = {
      ...zeroUsage,
      input: tokensUsed,
      totalTokens: tokensUsed,
    };
    const children = (await this.getRlmChildSnapshots()).map((child) => {
      const c = child as {
        id?: string;
        label?: string;
        status?: string;
      };
      const statusRaw = String(c.status ?? "done").toLowerCase();
      const status =
        statusRaw === "queued"
          ? "queued"
          : statusRaw === "running"
            ? "running"
            : statusRaw === "error"
              ? "error"
              : statusRaw === "cancelled"
                ? "cancelled"
                : "done";
      return {
        id: String(c.id ?? "child"),
        label: String(c.label ?? c.id ?? "child"),
        status: status as ContextTreeNode["status"],
        ownUsage: { ...zeroUsage },
        totalUsage: { ...zeroUsage },
        children: [],
      };
    });
    return {
      id: "root",
      label: this.sessionName?.trim() || "session",
      status: "active" as const,
      model: this.model
        ? { provider: this.model.provider, id: this.model.id }
        : undefined,
      ownUsage,
      totalUsage: { ...ownUsage },
      contextUsage: this.contextUsage,
      children,
    };
  }

  async getSessionContext(): Promise<AgentConnectionSessionContext> {
    return {
      messages: await this.getMessages(),
      thinkingLevel: this.thinkingLevel,
      serviceTier: "default",
      model: this.model ? { provider: this.model.provider, modelId: this.model.id } : null,
    };
  }

  async getSessionTree(): Promise<{ tree: AgentConnectionSessionTreeNode[]; leafId: string | null }> {
    if (!this.sessionId) return { tree: [], leafId: null };
    const result = (await this.rpc.request("session/tree/read", {
      sessionId: this.sessionId,
    })) as {
      tree?: AgentConnectionSessionTreeNode[];
      leafId?: string | null;
    };
    return {
      tree: Array.isArray(result?.tree) ? result.tree : [],
      leafId: result?.leafId ?? null,
    };
  }

  private async buildSessionTreeFromUserItems(): Promise<{
    tree: Array<{ entry: Record<string, unknown>; label?: string; children: unknown[] }>;
    leafId: string | null;
  }> {
    // Kept for tests; production uses session/tree/read.
    return { tree: [], leafId: null };
  }

  async listSavedSessions(
    scope: AgentConnectionSavedSessionScope = "all",
    callbacks?: AgentConnectionSessionListCallbacks,
  ): Promise<AgentConnectionSavedSessionInfo[]> {
    try {
      const sessions: Array<Record<string, unknown>> = [];
      let cursor: string | undefined;
      do {
        const page = (await this.rpc.request("session/list", {
          ...(scope === "current" ? { cwds: [this.cwd] } : {}),
          ...(cursor ? { cursor } : {}),
          limit: 200,
        })) as { data?: Array<Record<string, unknown>>; nextCursor?: string | null };
        if (Array.isArray(page?.data)) sessions.push(...page.data);
        cursor = page?.nextCursor ?? undefined;
      } while (cursor);
      const timestamp = (value: unknown): Date => {
        const date = new Date(typeof value === "string" || typeof value === "number" ? value : 0);
        return Number.isNaN(date.getTime()) ? new Date(0) : date;
      };
      const mapped: AgentConnectionSavedSessionInfo[] = sessions.map((s) => ({
        // `id` is the reconcile merge key (`session:<id>` alias): without it
        // every saved record aliases to "session:undefined" and never merges
        // with its daemon roster twin.
        id: String(s.id ?? ""),
        path: String(s.id ?? ""),
        sessionId: String(s.id ?? ""),
        name: (s.title ?? s.name) as string | undefined,
        firstMessage: (s.preview ?? s.firstMessage ?? "") as string,
        cwd: String(s.cwd ?? s.directory ?? this.cwd),
        modified: timestamp(s.lastActivityAt ?? s.updatedAt ?? s.updated_at ?? s.createdAt),
        created: timestamp(s.createdAt ?? s.created_at),
        messageCount: Number(s.messageCount ?? s.message_count ?? 0),
        allMessagesText: String(s.preview ?? s.firstMessage ?? ""),
        state: { status: "active" as const },
      }));
      for (const session of mapped) callbacks?.onSession?.(session);
      callbacks?.onProgress?.(mapped.length, mapped.length);
      return mapped;
    } catch {
      return [];
    }
  }

  async subscribeAgentRoster(listener: () => void) {
    this.rosterListeners.add(listener);
    const roster = await subscribeNativeAgentRoster(
      {
        request: (method, params) => this.rpc.request(method, params),
        onNotification: (handler) => this.rpc.onNotification(handler),
      },
      this.cwd,
      listener,
    );
    return {
      summaries: () => roster.summaries() as never,
      dispose: async () => {
        this.rosterListeners.delete(listener);
        await roster.dispose();
      },
    };
  }

  async getQueue(): Promise<AgentConnectionQueueState> {
    return { steering: [...this.queue.steering], followUp: [...this.queue.followUp] };
  }

  /** Reconcile local Follow-up/Steering previews with durable `session/queue/list`. */
  private async refreshQueueFromServer(sessionId: string | null = this.sessionId): Promise<void> {
    if (!sessionId || this.disposed) return;
    try {
      const result = (await this.rpc.request("session/queue/list", { sessionId })) as { entries?: unknown[] };
      if (this.sessionId !== sessionId || this.disposed) return;
      const entries = Array.isArray(result?.entries) ? result.entries : [];
      this.queueEntries = parseQueueWireEntries(entries);
      const followUp: string[] = [];
      for (const entry of entries) {
        if (!entry || typeof entry !== "object") continue;
        const record = entry as Record<string, unknown>;
        const preview = String(record.preview ?? "").trim();
        if (preview) {
          followUp.push(preview);
          continue;
        }
        const input = Array.isArray(record.input) ? record.input : [];
        const parts: string[] = [];
        for (const part of input) {
          if (!part || typeof part !== "object") continue;
          const text = (part as { text?: unknown }).text;
          if (text != null) parts.push(String(text));
        }
        const joined = parts.join("\n").trim();
        if (joined) followUp.push(joined);
      }
      // Steering is turn-scoped and not listed on the durable queue.
      this.queue = { steering: [...this.queue.steering], followUp };
      this.syncActionsFromQueue();
      this.emitSessionEvent({
        type: "session_action_update",
        actions: this.buildState().sessionActions,
      });
    } catch {
      // Best-effort; keep last known local preview.
    }
  }
  async mutateQueuedMessage(
    lane: "steering" | "followUp",
    index: number,
    expectedText: string,
    mutation: { type: "delete" } | { type: "move"; direction: -1 | 1 } | { type: "replace"; text: string; images?: unknown[]; lane: "steering" | "followUp" },
  ) {
    if (lane === "steering") {
      // Steering previews are local until the server drains them; mutate locally.
      if (this.queue.steering[index] !== expectedText) return "rejected" as const;
      if (mutation.type === "delete") {
        this.queue.steering.splice(index, 1);
        this.syncActionsFromQueue();
        return "applied" as const;
      }
      if (mutation.type === "move") {
        const swap = index + mutation.direction;
        if (swap < 0 || swap >= this.queue.steering.length) return "invalid" as const;
        const tmp = this.queue.steering[index]!;
        this.queue.steering[index] = this.queue.steering[swap]!;
        this.queue.steering[swap] = tmp;
        this.syncActionsFromQueue();
        return "applied" as const;
      }
      if (mutation.type === "replace") {
        this.queue.steering[index] = mutation.text;
        if (mutation.lane === "followUp") {
          this.queue.steering.splice(index, 1);
          this.queue.followUp.push(mutation.text);
        }
        this.syncActionsFromQueue();
        return "applied" as const;
      }
      return "invalid" as const;
    }

    try {
      await this.refreshQueueFromServer();
    } catch {
      return "unsupported" as const;
    }
    const entry = this.queueEntries[index];
    if (!entry || queuePreviewText(entry) !== expectedText) return "rejected" as const;
    const sessionId = this.requireSessionId();
    try {
      if (mutation.type === "delete") {
        await this.rpc.request("session/queue/remove", {
          sessionId,
          queueItemId: entry.queueItemId,
        });
        await this.refreshQueueFromServer();
        return "applied" as const;
      }
      if (mutation.type === "move") {
        const nextPos = entry.position + mutation.direction;
        if (nextPos < 0) return "invalid" as const;
        await this.rpc.request("session/queue/update", {
          sessionId,
          queueItemId: entry.queueItemId,
          position: nextPos,
        });
        await this.refreshQueueFromServer();
        return "applied" as const;
      }
      if (mutation.type === "replace") {
        await this.rpc.request("session/queue/update", {
          sessionId,
          queueItemId: entry.queueItemId,
          input: textInput(mutation.text),
        });
        if (mutation.lane === "steering") {
          await this.rpc.request("session/queue/remove", {
            sessionId,
            queueItemId: entry.queueItemId,
          });
          this.queue.steering.push(mutation.text);
        }
        await this.refreshQueueFromServer();
        return "applied" as const;
      }
      return "invalid" as const;
    } catch {
      return "rejected" as const;
    }
  }

  async clearQueue() {
    const sessionId = this.sessionId;
    if (sessionId) {
      try {
        const result = (await this.rpc.request("session/queue/list", {
          sessionId,
        })) as { entries?: unknown };
        const entries = parseQueueWireEntries(result?.entries);
        for (const entry of entries) {
          await this.rpc.request("session/queue/remove", {
            sessionId,
            queueItemId: entry.queueItemId,
          });
        }
      } catch {
        // fall through to local clear
      }
    }
    this.queueEntries = [];
    this.queue = { steering: [], followUp: [] };
    this.syncActionsFromQueue();
    this.emitSessionEvent({ type: "session_action_update", actions: this.buildState().sessionActions });
    return this.getQueue();
  }

  async abortAndClearQueue() {
    await this.abort();
    return this.clearQueue();
  }

  async acquireSessionInputPause() {
    return { release: async () => {} };
  }

  async listCronJobs(options?: { includeInactive?: boolean }) {
    const result = (await this.rpc.request("session/schedule/list", {
      sessionId: this.sessionId ?? undefined,
      cwd: this.cwd,
    })) as { jobs?: ScheduleJobWire[] };
    const jobs = (result?.jobs ?? []).filter((j) => j.kind === "cron");
    if (options?.includeInactive) return jobs.map(toAgentCronJob);
    return jobs
      .filter((j) => j.status === "active" || j.status === "paused")
      .map(toAgentCronJob);
  }
  async listHeartbeats() {
    const result = (await this.rpc.request("session/schedule/list", {
      cwd: this.cwd,
    })) as { jobs?: ScheduleJobWire[] };
    return (result?.jobs ?? [])
      .filter((j) => j.kind === "heartbeat" && j.status !== "stopped")
      .map((j) => ({ job: toAgentCronJob(j) }));
  }
  async manageHeartbeat(_activeSessionId: string, jobId: string, action: "pause" | "resume" | "stop") {
    const result = (await this.rpc.request("session/schedule/update", {
      jobId,
      action,
    })) as { job: ScheduleJobWire };
    return toAgentCronJob(result.job);
  }
  async addCronJob(schedule: string, prompt: string) {
    const result = (await this.rpc.request("session/schedule/upsert", {
      kind: "cron",
      sessionId: this.requireSessionId(),
      cwd: this.cwd,
      schedule,
      prompt,
      deliveryMode: "steer",
    })) as { job: ScheduleJobWire };
    return toAgentCronJob(result.job);
  }
  async cancelCronJob(jobId: string) {
    const result = (await this.rpc.request("session/schedule/delete", {
      jobId,
    })) as { job?: ScheduleJobWire };
    if (result?.job) return toAgentCronJob(result.job);
    return {
      id: jobId,
      status: "cancelled" as const,
      activeSessionId: this.sessionId ?? "",
      sessionId: this.sessionId ?? "",
      sessionFile: "",
      cwd: this.cwd,
      prompt: "",
      schedule: { kind: "cron" as const, expression: "" },
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      runCount: 0,
    };
  }
  async getHeartbeat() {
    const heartbeats = await this.listHeartbeats();
    const mine = heartbeats.find(
      (h) =>
        (h.job.sessionId === this.sessionId || h.job.activeSessionId === this.sessionId) &&
        !h.job.label,
    );
    return mine?.job;
  }
  async runHeartbeatCommand(args: string) {
    const result = (await this.rpc.request("session/heartbeat/command", {
      sessionId: this.requireSessionId(),
      cwd: this.cwd,
      args,
    })) as { action: string; job?: ScheduleJobWire };
    return {
      action: result.action as "status" | "set" | "pause" | "resume" | "clear",
      job: result.job ? toAgentCronJob(result.job) : undefined,
    };
  }
  async setHeartbeat(
    schedule: string,
    instruction: string,
    deliveryMode?: "steer" | "follow_up",
  ) {
    const result = await this.runHeartbeatCommand(
      [
        deliveryMode === "follow_up" ? "--follow-up" : "",
        schedule,
        instruction,
      ]
        .filter(Boolean)
        .join(" "),
    );
    if (!result.job) {
      throw new Error("Heartbeat was not set");
    }
    return result.job;
  }
  async updateHeartbeat(action: "pause" | "resume" | "clear") {
    const result = await this.runHeartbeatCommand(action === "clear" ? "clear" : action);
    return result.job;
  }
  async sendAgentMessage(targetActiveSessionId: string, message: string) {
    const targetSessionId = String(targetActiveSessionId);
    // Prefer inject-into-turn when the target matches the active turn session.
    if (targetSessionId === this.sessionId && this.expectedTurnId) {
      try {
        const result = (await this.rpc.request("turn/steer", {
          sessionId: targetSessionId,
          expectedTurnId: this.expectedTurnId,
          input: textInput(message),
          idempotencyKey: newIdempotencyKey("agent-msg"),
        })) as { outcome?: string };
        const outcome = String(result?.outcome ?? "").toLowerCase();
        return newAgentMessageReceipt({
          targetSessionId,
          message,
          fromSessionId: this.sessionId ?? undefined,
          deliveryStatus: outcome === "degradedtoqueue" ? "queued" : "delivered",
        }) as never;
      } catch {
        // fall through to queue
      }
    }
    await this.rpc.request("session/queue/push", {
      sessionId: targetSessionId,
      input: textInput(message),
      idempotencyKey: newIdempotencyKey("agent-msg"),
    });
    return newAgentMessageReceipt({
      targetSessionId,
      message,
      fromSessionId: this.sessionId ?? undefined,
      deliveryStatus: "queued",
    }) as never;
  }

  async getAgentMessageStatus() {
    return {
      paused: false,
      pending: 0,
      maxMessageChars: 16_384,
      maxPendingPerSession: 20,
      rateLimitCapacity: 3,
      rateLimitRefillMs: 1000,
    };
  }

  async pauseAgentMessages() {
    return this.getAgentMessageStatus();
  }

  async resumeAgentMessages() {
    return this.getAgentMessageStatus();
  }
  async clearAgentMessages() {
    return 0;
  }

  async getUserMessagesForForking() {
    const envelopes = await this.listAllItemEnvelopes();
    // Scope to the active root→leaf path: a linear items/list also carries
    // abandoned branches, which the server refuses to cut from. `null` means
    // the tree is unavailable — fall back to the linear list.
    const activeItemIds = await this.activePathItemIds();
    const out: Array<{ entryId: string; text: string }> = [];
    const seenTurns = new Set<string>();
    for (const env of envelopes) {
      const item = (env.item ?? env) as Record<string, unknown>;
      const type = String(item.type ?? "").toLowerCase();
      if (type !== "usermessage" && type !== "user_message") continue;
      const itemId = String(env.id ?? item.id ?? "");
      if (activeItemIds && itemId && !activeItemIds.has(itemId)) continue;
      const turnId = String(env.turnId ?? env.turn_id ?? "");
      if (!turnId || seenTurns.has(turnId)) continue;
      seenTurns.add(turnId);
      out.push({ entryId: turnId, text: extractText(item.content) });
    }
    return out;
  }

  /**
   * Item ids on the active root→leaf transcript path (`session/tree/read`).
   * Walks parent links from the leaf; `null` when the leaf or tree cannot
   * be resolved (callers then keep the linear list).
   */
  private async activePathItemIds(): Promise<Set<string> | null> {
    try {
      const { tree, leafId } = await this.getSessionTree();
      if (!leafId) return null;
      const parents = new Map<string, string | null>();
      const walk = (nodes: Array<{ entry?: unknown; children?: unknown }>) => {
        for (const node of nodes) {
          const entry = (node?.entry ?? {}) as Record<string, unknown>;
          const id = String(entry.id ?? "");
          if (id) {
            parents.set(id, entry.parentId == null ? null : String(entry.parentId));
          }
          if (Array.isArray(node?.children)) {
            walk(node.children as Array<{ entry?: unknown; children?: unknown }>);
          }
        }
      };
      walk(tree);
      const active = new Set<string>();
      let current: string | null = leafId;
      while (current && !active.has(current)) {
        active.add(current);
        current = parents.get(current) ?? null;
      }
      return active.size > 0 ? active : null;
    } catch {
      return null;
    }
  }

  async getLastAssistantText() {
    const messages = await this.getMessages();
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i] as { role?: string; content?: unknown };
      if (m.role === "assistant") return extractText(m.content);
    }
    return undefined;
  }

  async getSystemPrompt() {
    const sessionId = this.requireSessionId();
    try {
      const result = (await this.rpc.request("session/systemPrompt/read", {
        sessionId,
      })) as { prompt?: string };
      if (typeof result?.prompt === "string" && result.prompt.length > 0) {
        return result.prompt;
      }
    } catch {
      // fall through
    }
    return "(system prompt unavailable — session/systemPrompt/read failed)";
  }

  async getToolDefinition(name: string) {
    if (name === "ipython") {
      return {
        name: "ipython",
        label: formatNativeToolLabel(name),
        description: "Execute Python code in the session RLM kernel.",
        parameters: {
          type: "object",
          properties: { code: { type: "string" } },
          required: ["code"],
        },
        renderShell: "self" as const,
      };
    }
    if (name === "bash" || name === "exec_command" || name === "shell_command" || name === "command") {
      return {
        name: "bash",
        label: "bash",
        description: "Execute a shell command.",
        parameters: {
          type: "object",
          properties: { command: { type: "string" } },
          required: ["command"],
        },
        replayBuiltInToolName: "bash" as const,
      };
    }
    if (name === "edit" || name === "write" || name === "apply_patch" || name === "file_change") {
      return {
        name: "edit",
        label: "edit",
        description: "Edit a file.",
        parameters: {
          type: "object",
          properties: {
            path: { type: "string" },
            oldText: { type: "string" },
            newText: { type: "string" },
          },
          required: ["path"],
        },
        replayBuiltInToolName: "edit" as const,
      };
    }
    // Generic definition so ToolExecutionComponent can render unknown tools
    // without crashing on a missing schema.
    return {
      name,
      label: formatNativeToolLabel(name),
      description: "",
      parameters: { type: "object", properties: {} },
    };
  }

  async setSessionEntryLabel() {}

  async respondToExtensionUiRequest(
    requestId: string,
    response: AgentConnectionExtensionUiResponse,
  ): Promise<void> {
    const pending = this.pendingReverse.get(requestId);
    this.pendingReverse.delete(requestId);
    const numId = Number(requestId);
    const id: JsonRpcId = Number.isFinite(numId) && String(numId) === requestId ? numId : requestId;
    const method = pending?.method ?? "";
    const params = (pending?.params && typeof pending.params === "object"
      ? pending.params
      : {}) as Record<string, unknown>;
    const wireRequestId = String(params.requestId ?? params.approvalId ?? requestId);
    const decidedAt = new Date().toISOString();

    if ("cancelled" in response && response.cancelled) {
      if (method.startsWith("approval/") || method === "session/goal/completionApproval/request") {
        this.rpc.respond(id, {
          requestId: wireRequestId,
          decision: {
            decision: "cancelled",
            scope: "once",
            decisionSource: "user",
            decidedAt,
          },
        });
        return;
      }
      this.rpc.respondError(id, "cancelled", -32001);
      return;
    }

    if ("confirmed" in response) {
      this.rpc.respond(id, {
        requestId: wireRequestId,
        decision: {
          decision: response.confirmed ? "approved" : "denied",
          scope: "once",
          decisionSource: "user",
          decidedAt,
        },
      });
      return;
    }

    if (method.startsWith("approval/") && "value" in response) {
      // Scope selection: the user picked one of the server-provided option
      // labels; map it back to the option id and the wire scope it carries.
      const chosen = String(response.value ?? "");
      const options = Array.isArray(params.options)
        ? (params.options as Array<Record<string, unknown>>)
        : [];
      const chosenOption = options.find(
        (option) => typeof option.name === "string" && option.name === chosen,
      );
      const optionId = typeof chosenOption?.option_id === "string" ? chosenOption.option_id : "";
      const denied = optionId === "reject_once" || optionId === "";
      this.rpc.respond(id, {
        requestId: wireRequestId,
        decision: {
          decision: denied ? "denied" : "approved",
          scope: approvalScopeForOptionId(optionId),
          decisionSource: "user",
          decidedAt,
        },
      });
      return;
    }

    if ("value" in response) {
      const questions = Array.isArray(params.questions) ? params.questions : [];
      const first = (questions[0] ?? {}) as { id?: string };
      const questionId = String(first.id ?? "q0");
      this.rpc.respond(id, {
        requestId: wireRequestId,
        answers: {
          [questionId]: { answers: [String(response.value ?? "")] },
        },
      });
      return;
    }

    if ("selected" in response || "optionId" in (response as object)) {
      const selected = String(
        (response as { selected?: string; optionId?: string; value?: string }).selected ??
          (response as { optionId?: string }).optionId ??
          "",
      );
      const questions = Array.isArray(params.questions) ? params.questions : [];
      const first = (questions[0] ?? {}) as { id?: string };
      const questionId = String(first.id ?? "q0");
      this.rpc.respond(id, {
        requestId: wireRequestId,
        answers: {
          [questionId]: { answers: selected ? [selected] : [] },
        },
      });
      return;
    }

    this.rpc.respond(id, response);
  }

  async prompt(message: string, options: AgentConnectionPromptOptions = {}): Promise<void> {
    if (await this.tryHandleGoalSlash(message)) return;
    if (await this.tryHandlePermissionsSlash(message)) return;
    if (await this.tryHandleCollaborationModeSlash(message)) return;
    if (await this.tryHandleTracesSlash(message)) return;
    if (await this.tryHandleSessionSlash(message)) return;

    // InteractiveMode always passes streamingBehavior ("steer" by default)
    // with resumeIfIdle semantics: idle → start a turn; busy → steer/queue.
    if (options.streamingBehavior === "steer") {
      if (this.isStreaming && this.expectedTurnId) {
        await this.steer(message, options.images);
        return;
      }
      // Busy but turn id lost (e.g. after a partial abort): queue, don't turn/start.
      if (this.isStreaming) {
        await this.followUp(message, options.images);
        return;
      }
      await this.startTurn(message, options.images);
      return;
    }
    if (options.streamingBehavior === "followUp" || options.queueIfBusy) {
      if (this.isStreaming) {
        await this.followUp(message, options.images);
        return;
      }
      await this.startTurn(message, options.images);
      return;
    }
    await this.startTurn(message, options.images);
  }

  private async startTurn(message: string, images?: unknown[]): Promise<void> {
    const sessionId = this.requireSessionId();
    this.isStreaming = true;
    const input = promptInput(message, images);
    try {
      const result = (await this.rpc.request("turn/start", {
        sessionId,
        input,
        idempotencyKey: newIdempotencyKey("prompt"),
      })) as { turn?: { id?: string } };
      if (result?.turn?.id) this.expectedTurnId = String(result.turn.id);
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      if (/already has an active prompt turn/i.test(msg)) {
        // Server still busy; client thought idle. Queue as follow-up instead of failing.
        this.isStreaming = true;
        await this.followUp(message, images);
        return;
      }
      this.isStreaming = false;
      this.expectedTurnId = null;
      throw error;
    }
  }

  /**
   * AgentSession treats compact/refine/goal/autonomous as session commands
   * before turn/start. InteractiveMode does not handle compact/refine itself when
   * bindLocalSessionExtensions is false — they arrive here via prompt().
   */
  private async tryHandleSessionSlash(message: string): Promise<boolean> {
    const trimmed = message.trim();
    if (!trimmed.startsWith("/") || /[\r\n\u2028\u2029]/.test(trimmed)) return false;
    const match = /^\/([^\s]+)(?:\s+([\s\S]*))?$/.exec(trimmed);
    if (!match) return false;
    const name = match[1]!.toLowerCase();
    const args = (match[2] ?? "").trim();
    if (name === "compact") {
      try {
        await this.compact(args || undefined);
      } catch (error) {
        // Session-command path swallows errors the connection already painted
        // via compaction_end — skip warnings, and the turn-active rejection
        // compact() emits as an error outcome before rethrowing. Letting the
        // error escape duplicates the row in the submit path's showError.
        const msg = error instanceof Error ? error.message : String(error);
        if (/too short|already compacted|cannot compact while a turn is active/i.test(msg)) {
          return true;
        }
        throw error;
      }
      return true;
    }
    if (name === "refine") {
      await this.refine({ instructions: args || undefined });
      return true;
    }
    if (name === "autonomous") {
      throw new Error(
        "/autonomous is not wired on Native yet; use /goal for persistent objectives.",
      );
    }
    return false;
  }

  private async tryHandleGoalSlash(message: string): Promise<boolean> {
    const action = parseGoalSlash(message);
    if (!action) return false;
    const sessionId = this.requireSessionId();
    if (action.kind === "read") {
      // origin: "user" — explicit /goal commands always announce their result,
      // even when the goal state is unchanged (e.g. bare /goal with no goal).
      const result = (await this.rpc.request("session/goal/read", {
        sessionId,
      })) as { goal?: unknown };
      this.goal = applyGoalFromWire(result?.goal) as never;
      this.emitSessionEvent({ type: "goal_update", goal: this.goal, origin: "user" });
      return true;
    }
    if (action.kind === "set") {
      const result = (await this.rpc.request("session/goal/set", {
        sessionId,
        objective: action.objective,
        ifExists: "replace",
        idempotencyKey: newIdempotencyKey("goal"),
      })) as { goal?: unknown };
      this.goal = applyGoalFromWire(result?.goal) as never;
      this.emitSessionEvent({ type: "goal_update", goal: this.goal, origin: "user" });
      return true;
    }
    const current = (await this.rpc.request("session/goal/read", {
      sessionId,
    })) as { goal?: { id?: string } | null };
    const expectedGoalId = current.goal?.id;
    if (!expectedGoalId) {
      this.goal = emptyGoalState() as never;
      this.emitSessionEvent({ type: "goal_update", goal: this.goal, origin: "user" });
      return true;
    }
    const result = (await this.rpc.request(action.method, {
      sessionId,
      expectedGoalId,
    })) as { goal?: unknown };
    this.goal =
      action.method === "session/goal/clear"
        ? (emptyGoalState() as never)
        : (applyGoalFromWire(result?.goal ?? { id: expectedGoalId }) as never);
    this.emitSessionEvent({ type: "goal_update", goal: this.goal, origin: "user" });
    return true;
  }

  private notifyStatus(message: string): void {
    this.emit({
      type: "extension_ui_request",
      request: {
        id: `notify_${Date.now().toString(36)}`,
        method: "notify",
        payload: { message },
      } as never,
    });
  }

  private async tryHandlePermissionsSlash(message: string): Promise<boolean> {
    const action = parsePermissionsSlash(message);
    if (!action) {
      if (/^\/permissions\b/i.test(message.trim())) {
        throw new Error("Usage: /permissions [default|autoReview|fullAccess]");
      }
      return false;
    }
    if (action.kind === "read") {
      await this.refreshPermissionProfile();
      this.notifyStatus(`Permission profile: ${this.permissionProfile}`);
      return true;
    }
    await this.updateSessionMetadata({
      settings: { permissionProfile: action.profile },
    });
    this.permissionProfile = action.profile;
    this.notifyStatus(`Permission profile set: ${this.permissionProfile}`);
    return true;
  }

  /** `/plan` / `/build` — session collaboration mode via SessionSettingsPatch.mode. */
  private async tryHandleCollaborationModeSlash(message: string): Promise<boolean> {
    const trimmed = message.trim();
    const match = /^\/(plan|build)\s*$/i.exec(trimmed);
    if (!match) return false;
    const mode = match[1]!.toLowerCase() === "plan" ? "plan" : "build";
    await this.updateSessionMetadata({
      settings: { mode },
    });
    this.notifyStatus(
      mode === "plan"
        ? "Collaboration mode: plan (mutating tools denied)"
        : "Collaboration mode: build",
    );
    return true;
  }

  /** `/traces` — status of, and runtime control over, the local protocol trace. */
  private async tryHandleTracesSlash(message: string): Promise<boolean> {
    const action = parseTracesSlash(message);
    if (!action) {
      if (/^\/traces\b/i.test(message.trim())) {
        throw new Error("Usage: /traces [status|on|off|preview [count]]");
      }
      return false;
    }
    const log = this.trafficLog;
    if (!log) {
      throw new Error("Native traffic tracing is unavailable in this host.");
    }
    if (action.kind === "enable") {
      const state = log.enable();
      this.notifyStatus(
        state.enabled && state.path
          ? `Traces enabled: ${state.path}`
          : "Traces could not be enabled.",
      );
      return true;
    }
    if (action.kind === "disable") {
      log.disable();
      this.notifyStatus("Traces disabled");
      return true;
    }
    if (action.kind === "preview") {
      const records = log.preview(action.limit);
      if (records.length === 0) {
        this.notifyStatus("No trace records found.");
        return true;
      }
      this.notifyStatus(
        [`Traces preview (${records.length} records):`, ...records.map(formatTracePreviewRecord)].join(
          "\n",
        ),
      );
      return true;
    }
    const state = log.getState();
    const files = log.list();
    this.notifyStatus(
      [
        `Traces: ${state.enabled ? "on" : "off"}`,
        `File: ${state.path ?? "(none)"}`,
        `Trace files: ${files.length}`,
        "Commands: /traces [status|on|off|preview [count]]",
      ].join("\n"),
    );
    return true;
  }

  async promptAndWait(message: string, options?: AgentConnectionPromptOptions): Promise<void> {
    await this.prompt(message, options);
    await this.waitForIdle();
  }

  async startSideQuestion(_id: string, question: string): Promise<void> {
    // No Native side-question RPC; surface as an ephemeral labeled prompt.
    await this.prompt(`/btw ${question}`);
  }

  async abortSideQuestion(): Promise<boolean> {
    return false;
  }

  async steer(message: string, _images?: unknown[]): Promise<void> {
    // resumeIfIdle: no active turn → start one.
    // Busy path: Native `turn/steer` injects raw input into the active turn.
    if (!this.isStreaming || !this.expectedTurnId) {
      await this.startTurn(message, _images);
      return;
    }
    const sessionId = this.requireSessionId();
    this.queue.steering.push(message);
    this.syncActionsFromQueue();
    this.emitSessionEvent({ type: "session_action_update", actions: this.buildState().sessionActions });
    try {
      const input = promptInput(message, _images);
      const result = (await this.rpc.request("turn/steer", {
        sessionId,
        expectedTurnId: this.expectedTurnId,
        input,
        idempotencyKey: newIdempotencyKey("steer"),
      })) as { outcome?: string; entry?: unknown };
      if (String(result?.outcome ?? "").toLowerCase() === "degradedtoqueue") {
        this.queue.followUp.push(message);
      }
    } catch (error) {
      // The server rejects steering a non-regular turn (e.g. a manual
      // compaction in flight) expecting clients to queue instead. Degrade to
      // the follow-up queue — the message is never lost and runs once the
      // session is idle again.
      const code = (error as { code?: unknown })?.code;
      if (
        code === "ActiveTurnNotSteerable" ||
        /cannot steer a non-regular turn/i.test(String((error as Error)?.message ?? ""))
      ) {
        await this.followUp(message, _images);
        return;
      }
      throw error;
    } finally {
      this.queue.steering = this.queue.steering.filter((text) => text !== message);
      this.syncActionsFromQueue();
      this.emitSessionEvent({ type: "session_action_update", actions: this.buildState().sessionActions });
    }
  }

  async followUp(message: string, _images?: unknown[]): Promise<void> {
    const sessionId = this.requireSessionId();
    this.queue.followUp.push(message);
    this.syncActionsFromQueue();
    this.emitSessionEvent({ type: "session_action_update", actions: this.buildState().sessionActions });
    try {
      await this.rpc.request("session/queue/push", {
        sessionId,
        input: promptInput(message, _images),
        idempotencyKey: newIdempotencyKey("queue"),
      });
    } catch (error) {
      // Roll back the optimistic entry: the server never accepted it, and a
      // phantom queue row would linger until the next queue/updated event.
      this.queue.followUp = this.queue.followUp.filter((text) => text !== message);
      this.syncActionsFromQueue();
      this.emitSessionEvent({ type: "session_action_update", actions: this.buildState().sessionActions });
      throw error;
    }
  }

  async abort(): Promise<void> {
    if (!this.sessionId) {
      this.isStreaming = false;
      this.resolveIdleWaiters();
      return;
    }
    // Clear Executing chrome immediately — session/interrupt waits up to 5s for
    // terminal turn status, which left Esc looking like a no-op mid-tool.
    this.localAbortChromePending = true;
    for (const ev of abortUiSessionEvents(this.streamState)) {
      this.emitSessionEvent(ev);
    }
    this.isStreaming = false;
    this.expectedTurnId = null;
    this.resolveIdleWaiters();
    // Don't leave a stale/wrong turn workspace recap after Esc (D03 on interrupt).
    void this.seedWorkspaceFileChangesRecap();

    const interrupt = this.rpc.request("session/interrupt", {
      scope: { scope: "session", sessionId: this.sessionId },
    });
    try {
      await Promise.race([
        interrupt,
        new Promise<void>((_, reject) => {
          setTimeout(() => reject(new Error("session/interrupt timed out")), 10_000);
        }),
      ]);
    } catch {
      // Server may finish asynchronously; UI already cleared.
    }
  }

  async cancelRlmChild(): Promise<boolean> {
    return false;
  }

  async waitForIdle(): Promise<void> {
    if (!this.isStreaming) return;
    await new Promise<void>((resolve) => {
      this.idleWaiters.add(resolve);
    });
  }

  async waitForHeadlessCompletion() {
    await this.waitForIdle();
    return { enabled: false };
  }

  async executeBash(
    command: string,
    options?: { cwd?: string; excludeFromContext?: boolean; transient?: boolean; runId?: string },
  ): Promise<void> {
    const sessionId = this.requireSessionId();
    this.isBashRunning = true;
    this.lastStartedBashItemId = null;
    // Mount `$ <cmd>` once locally. command/exec deltas have no command field
    // and must not synthesize a second bash_start while this flag is set.
    this.streamState.userBashUiMounted = true;
    const runId = options?.runId;
    this.emitSessionEvent({
      type: "bash_start",
      command,
      excludeFromContext: Boolean(options?.excludeFromContext ?? true),
      transient: Boolean(options?.transient),
      runId,
    });
    try {
      const result = (await this.rpc.request("task/start", {
        kind: "process",
        sessionId,
        command,
        cwd: options?.cwd ?? this.cwd,
        idempotencyKey: newIdempotencyKey("bash"),
      })) as { itemId?: string; item_id?: string };
      const itemId = String(result?.itemId ?? result?.item_id ?? "") || null;
      this.lastStartedBashItemId = itemId;
      if (this.isBashRunning) {
        this.activeBashItemId = itemId;
        if (itemId) this.streamState.activeBashRuns.add(itemId);
      }
    } catch (error) {
      this.isBashRunning = false;
      this.activeBashItemId = null;
      this.streamState.userBashUiMounted = false;
      this.emitSessionEvent({
        type: "bash_end",
        exitCode: undefined,
        cancelled: false,
        truncated: false,
        errorMessage: error instanceof Error ? error.message : String(error),
        runId,
        transient: Boolean(options?.transient),
      });
      throw error;
    }
  }

  async executeBashAndWait(command: string): Promise<BashResult> {
    await this.executeBash(command);
    // bash_end can clear activeBashItemId before executeBash resolves.
    const itemId = this.lastStartedBashItemId;
    await this.waitForBashIdle();
    if (!itemId) {
      throw new Error("task/start did not return an itemId");
    }
    const read = (await this.rpc.request("task/read", {
      itemId,
    })) as { outputTail?: string; item?: { status?: string; exitCode?: number; item?: { exitCode?: number } } };
    const exitCode = read?.item?.exitCode ?? read?.item?.item?.exitCode;
    return {
      output: String(read?.outputTail ?? ""),
      exitCode: typeof exitCode === "number" ? exitCode : undefined,
      cancelled: read?.item?.status === "cancelled",
      truncated: false,
    };
  }

  async abortBash(): Promise<void> {
    if (!this.activeBashItemId) {
      this.isBashRunning = false;
      return;
    }
    try {
      await this.rpc.request("task/interrupt", { itemId: this.activeBashItemId });
    } catch {
      // best-effort
    }
    this.isBashRunning = false;
    this.activeBashItemId = null;
  }

  private async waitForBashIdle(): Promise<void> {
    if (!this.isBashRunning) return;
    await new Promise<void>((resolve) => {
      const check = () => {
        if (!this.isBashRunning) {
          resolve();
          return;
        }
        setTimeout(check, 50);
      };
      check();
    });
  }

  async setModel(provider: string, modelId: string): Promise<AgentConnectionModel> {
    await this.updateSessionMetadata({
      model: { provider, model: modelId },
    });
    const catalog = await this.getAvailableModels().catch(() => [] as AgentConnectionModel[]);
    const fromCatalog =
      findCatalogModel(catalog, { id: modelId, provider }) ??
      catalog.find((m) => m.id === modelId);
    const model =
      fromCatalog ??
      normalizeAgentModel({ provider, id: modelId, modelId, reasoning: true })!;
    this.model = model;
    this.applyAvailableThinkingLevelsFromModel(model);
    // Model switch must rewrite the footer window immediately; occupancy refresh
    // can race and still carry the previous model's contextWindowTokens.
    this.ensureDefaultContextUsage();
    if (this.contextUsage) {
      this.emitSessionEvent({ type: "context_usage_update", contextUsage: this.contextUsage });
    }
    void this.refreshContextUsage();
    return model;
  }

  async cycleModel(direction: "forward" | "backward" = "forward") {
    const models = await this.getAvailableModels();
    if (models.length === 0) return undefined;
    const currentIdx = models.findIndex(
      (m) => m.id === this.model?.id && m.provider === this.model?.provider,
    );
    const base = currentIdx >= 0 ? currentIdx : 0;
    const nextIdx =
      direction === "backward"
        ? (base - 1 + models.length) % models.length
        : (base + 1) % models.length;
    const next = models[nextIdx]!;
    const model = await this.setModel(next.provider, next.id);
    return {
      model,
      thinkingLevel: this.thinkingLevel,
      serviceTier: "default" as const,
      isScoped: false,
    };
  }

  async setScopedModels(): Promise<void> {}

  async setThinkingLevel(level: ThinkingLevel): Promise<void> {
    this.thinkingLevel = level;
    try {
      await this.updateSessionMetadata({
        settings: {
          reasoningEffort: level,
        },
      });
    } catch {
      // best-effort
    }
    this.emitSessionEvent({ type: "thinking_level_changed", level });
  }

  async setServiceTier(): Promise<void> {}

  async cycleThinkingLevel(): Promise<ThinkingLevel | undefined> {
    const idx = this.availableThinkingLevels.indexOf(this.thinkingLevel);
    const next =
      this.availableThinkingLevels[(idx + 1) % this.availableThinkingLevels.length] ?? "medium";
    await this.setThinkingLevel(next);
    return next;
  }

  async setTransport(): Promise<void> {}

  async setSteeringMode(mode: "all" | "one-at-a-time"): Promise<void> {
    this.steeringMode = mode;
  }

  async setFollowUpMode(mode: "all" | "one-at-a-time"): Promise<void> {
    this.followUpMode = mode;
  }

  async setAutoCompactionEnabled(enabled: boolean): Promise<void> {
    this.autoCompactionEnabled = enabled;
  }

  async setAutoRetryEnabled(): Promise<void> {}

  async compact(customInstructions?: string) {
    const tooShortMessage = "Session is too short to compact — try again once it grows";
    const commandText = customInstructions?.trim()
      ? `/compact ${customInstructions.trim()}`
      : "/compact";
    const command = {
      name: "compact" as const,
      args: customInstructions?.trim() ?? "",
      text: commandText,
    };
    // Match /refine: paint the slash row so manual compaction is visible even when
    // Native compaction turns are quiet or skipped.
    this.emitSessionEvent({
      type: "message_start",
      message: {
        role: "custom",
        customType: "session_slash_command",
        content: commandText,
        display: true,
        details: { command },
        timestamp: Date.now(),
      } as never,
    });
    // prepareCompaction returns null for short branches before any model call.
    // Do the same locally so /compact always paints a durable warning without
    // depending on async Native compaction-turn notifications.
    const messages = await this.getMessages();
    if (messages.length < 4) {
      this.emitSessionEvent({
        type: "compaction_start",
        reason: "manual",
        customInstructions,
      });
      this.emitSessionEvent({
        type: "compaction_end",
        reason: "manual",
        result: undefined,
        aborted: false,
        willRetry: false,
        errorMessage: tooShortMessage,
        errorSeverity: "warning",
        customInstructions,
      });
      this.emitSessionEvent({
        type: "message_start",
        message: {
          role: "custom",
          customType: "session_slash_command_result",
          content: tooShortMessage,
          display: true,
          details: {
            command,
            success: false,
            severity: "warning",
            error: tooShortMessage,
          },
          timestamp: Date.now(),
        } as never,
      });
      throw new Error(tooShortMessage);
    }

    this.isCompacting = true;
    this.lastCompactionSkipMessage = null;
    this.emitSessionEvent({
      type: "compaction_start",
      reason: "manual",
      customInstructions,
    });
    try {
      // Native compact is async (admits a Compaction turn). Wait for the
      // projected compaction_end — do not treat turn/start as completion.
      await this.rpc.request("session/compact/start", {
        sessionId: this.requireSessionId(),
        customInstructions,
      });
      await this.waitForCompactionIdle();
      const skipMessage = this.lastCompactionSkipMessage;
      // Reload server history before painting the slash result so a transcript
      // refresh cannot wipe the durable /compact outcome row.
      await this.getMessages();
      if (skipMessage) {
        this.emitSessionEvent({
          type: "message_start",
          message: {
            role: "custom",
            customType: "session_slash_command_result",
            content: skipMessage,
            display: true,
            details: {
              command,
              success: false,
              severity: "warning",
              error: skipMessage,
            },
            timestamp: Date.now(),
          } as never,
        });
        throw new Error(skipMessage);
      }
      this.emitSessionEvent({
        type: "message_start",
        message: {
          role: "custom",
          customType: "session_slash_command_result",
          content: "Compaction finished.",
          display: true,
          details: {
            command,
            success: true,
            severity: "info",
          },
          timestamp: Date.now(),
        } as never,
      });
      return {
        summary: "",
        firstKeptEntryId: "",
        tokensBefore: 0,
        tokensAfter: 0,
      } as never;
    } catch (error) {
      this.isCompacting = false;
      const errorMessage = error instanceof Error ? error.message : String(error);
      const tooShort = /too short|skip|already compacted/i.test(errorMessage);
      if (tooShort && this.lastCompactionSkipMessage) {
        // Warning already emitted via projected compaction_end.
        throw error;
      }
      if (tooShort && errorMessage === tooShortMessage) {
        // Preflight path already emitted compaction_end.
        throw error;
      }
      this.emitSessionEvent({
        type: "compaction_end",
        reason: "manual",
        result: undefined,
        aborted: false,
        willRetry: false,
        errorMessage: tooShort ? tooShortMessage : errorMessage,
        errorSeverity: tooShort ? "warning" : "error",
        customInstructions,
      });
      throw error;
    }
  }

  private async waitForCompactionIdle(): Promise<void> {
    if (!this.isCompacting) return;
    await new Promise<void>((resolve) => {
      const started = Date.now();
      const check = () => {
        if (!this.isCompacting || Date.now() - started > 120_000) {
          resolve();
          return;
        }
        setTimeout(check, 50);
      };
      check();
    });
  }

  async refine(options?: { instructions?: string; rollbackId?: string; global?: boolean }) {
    const args = options?.instructions?.trim() ?? "";
    const commandText = args ? `/refine ${args}` : "/refine";
    const command = {
      name: "refine" as const,
      args,
      text: commandText,
    };
    // The TUI paints the /refine row + loader from session slash command messages.
    this.emitSessionEvent({
      type: "message_start",
      message: {
        role: "custom",
        customType: "session_slash_command",
        content: commandText,
        display: true,
        details: { command },
        timestamp: Date.now(),
      } as never,
    });
    try {
      const result = (await this.rpc.request("session/refine/run", {
        sessionId: this.requireSessionId(),
        instructions: options?.instructions,
        rollbackId: options?.rollbackId,
        global: options?.global,
      })) as {
        scheduled?: boolean;
        note?: string;
        reason?: string;
        refinementId?: string;
      };
      const scheduled = Boolean(result?.scheduled);
      const summary = scheduled
        ? (result.note ??
          "Refinement scheduled; it applies when the current turn ends (or immediately if idle).")
        : (result?.reason ?? "Refinement was not scheduled");
      this.emitSessionEvent({
        type: "message_start",
        message: {
          role: "custom",
          customType: "session_slash_command_result",
          content: summary,
          display: true,
          details: {
            command,
            success: scheduled,
            severity: scheduled ? "info" : "error",
            ...(scheduled ? {} : { error: summary }),
          },
          timestamp: Date.now(),
        } as never,
      });
      if (!scheduled) {
        this.emitSessionEvent({ type: "refine_failed", error: summary });
        const err = new Error(summary) as Error & { painted?: boolean };
        err.painted = true;
        throw err;
      }
      this.emitSessionEvent({
        type: "refine_complete",
        result: {
          id: result.refinementId ?? "",
          summary,
          scope: options?.global ? "global" : "local",
          appliedEdits: [],
        } as never,
      });
      return {
        id: result.refinementId ?? "",
        summary,
        scope: options?.global ? "global" : "local",
        appliedEdits: [],
      } as never;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      if (!(error instanceof Error && (error as Error & { painted?: boolean }).painted)) {
        this.emitSessionEvent({
          type: "message_start",
          message: {
            role: "custom",
            customType: "session_slash_command_result",
            content: `Command failed: ${errorMessage}`,
            display: true,
            details: {
              command,
              success: false,
              severity: "error",
              error: errorMessage,
            },
            timestamp: Date.now(),
          } as never,
        });
        this.emitSessionEvent({
          type: "refine_failed",
          error: errorMessage,
        });
      }
      throw error;
    }
  }

  async abortCompaction(): Promise<void> {
    this.isCompacting = false;
  }
  async abortBranchSummary(): Promise<void> {}
  async abortRetry(): Promise<void> {
    // Native path has no separate retry cancel RPC — interrupt the active turn.
    await this.abort();
  }
  async reload(): Promise<void> {
    await this.getMessages();
  }

  async newSession(_options?: AgentConnectionNewSessionOptions) {
    const transitionGeneration = ++this.sessionTransitionGeneration;
    this.fireBeforeInvalidate();
    const result = (await this.rpc.request("session/new", {
      cwd: this.cwd,
      idempotencyKey: randomUUID(),
    })) as { session?: { id?: string; title?: string; model?: unknown; settings?: unknown } };

    const session = result?.session;
    const id = String(session?.id ?? "");
    if (!id || transitionGeneration !== this.sessionTransitionGeneration) return { cancelled: true };

    this.pendingInitialMessages = undefined;
    this.sessionHistoryRevision += 1;
    this.sessionId = id;
    this.activeSessionId = id;
    this.sessionName = session?.title as string | undefined;
    this.messages = [];
    this.streamState = createStreamState();
    this.queue = { steering: [], followUp: [] };
    this.queueEntries = [];
    this.sessionActions = emptySessionActions();
    this.isStreaming = false;
    this.isCompacting = false;
    this.goal = emptyGoalState();
    // Fresh session has no occupancy yet — clear so the footer does not keep
    // the previous session's token % after /new.
    this.contextUsage = emptyContextUsage();
    this.ensureDefaultContextUsage();

    if (session?.model) {
      this.model = normalizeAgentModel(session.model as Record<string, unknown>) ?? this.model;
    }

    await this.createSubscription({ transitionGeneration });
    if (transitionGeneration !== this.sessionTransitionGeneration) return { cancelled: true };
    this.emit({
      type: "session_replaced",
      state: this.buildState(),
      messages: [],
    });
    return { cancelled: false };
  }

  async switchSession(sessionPath: string, options?: AgentConnectionSwitchSessionOptions) {
    const transitionGeneration = ++this.sessionTransitionGeneration;
    this.fireBeforeInvalidate();
    const sessionId = sessionPath;
    const resumed = (await this.rpc.request("session/resume", { sessionId })) as {
      session?: Record<string, unknown>;
      lastContextOccupancy?: unknown;
      recovery?: Record<string, unknown> | null;
    };
    const session = resumed?.session;
    if (!session?.id || transitionGeneration !== this.sessionTransitionGeneration) {
      return { cancelled: true };
    }

    const resumedSessionId = String(session.id);
    this.pendingInitialMessages = undefined;
    this.sessionHistoryRevision += 1;
    this.sessionId = resumedSessionId;
    this.activeSessionId = this.sessionId;
    this.sessionName = (session.title ?? session.name) as string | undefined;
    this.cwd = String(session.cwd ?? session.directory ?? this.cwd);
    this.messages = [];
    this.streamState = createStreamState();
    this.isStreaming = false;
    this.expectedTurnId = null;
    const resumedUsage = occupancyToContextUsage(resumed.lastContextOccupancy);
    this.contextUsage = (resumedUsage ?? emptyContextUsage()) as never;
    this.ensureDefaultContextUsage();
    if (session.model) {
      this.model = normalizeAgentModel(session.model as Record<string, unknown>) ?? this.model;
    }
    if (resumed.recovery && typeof resumed.recovery === "object") {
      this.emitSessionEvent({
        type: "session_lifecycle",
        action: "turn_recovery",
        recovery: resumed.recovery,
      });
    }
    const historyLoadMode = options?.historyLoadMode ?? "blocking";
    await this.createSubscription({
      supplementalRefreshMode: historyLoadMode === "deferred" ? "deferred" : "blocking",
      transitionGeneration,
    });
    if (transitionGeneration !== this.sessionTransitionGeneration) return { cancelled: true };
    // Abandoned InProgress + recovery must never leave Waiting chrome.
    if (resumed.recovery) {
      this.isStreaming = false;
      this.expectedTurnId = null;
    }
    let messages: never[];
    if (historyLoadMode === "deferred") {
      const revision = this.sessionHistoryRevision;
      const promise = this.getMessages();
      this.pendingInitialMessages = { sessionId: resumedSessionId, revision, promise };
      messages = [...this.messages] as never[];
      void this.refreshContextUsage(resumedSessionId);
      void promise.then(() => this.enrichModelFromCatalog(resumedSessionId)).catch(() => {});
      void this.refreshQueueFromServer(resumedSessionId).catch(() => {});
    } else {
      messages = await this.getMessages();
    }
    if (transitionGeneration !== this.sessionTransitionGeneration) return { cancelled: true };
    void this.seedWorkspaceFileChangesRecap();
    this.emit({
      type: "session_replaced",
      state: this.buildState(),
      messages: messages as never,
    });
    // Push the resumed occupancy immediately; deferred refresh will update it if absent.
    if (this.contextUsage) {
      this.emitSessionEvent({
        type: "context_usage_update",
        contextUsage: this.contextUsage,
      });
    }
    return { cancelled: false };
  }

  async fork(entryId: string, options?: AgentConnectionForkOptions) {
    this.fireBeforeInvalidate();
    const cut = options?.position === "before" ? "before" : "through";
    const atTurnId = await this.resolveForkTurnId(entryId);
    const result = (await this.rpc.request("session/fork", {
      sessionId: this.requireSessionId(),
      ...(atTurnId ? { atTurnId } : {}),
      cut,
    })) as { session?: { id?: string } };
    if (!result?.session?.id) {
      throw new Error("session/fork did not return a session");
    }
    await this.switchSession(String(result.session.id));
    return { cancelled: false };
  }

  /**
   * Native fork wants a TurnId (or tip when omitted). Session-tree leaf/entry
   * ids are ItemIds — map them, and treat the current leaf as a tip fork.
   */
  private async resolveForkTurnId(entryId: string): Promise<string | undefined> {
    const id = String(entryId ?? "").trim();
    if (!id) return undefined;
    if (id.startsWith("turn_")) return id;

    const tree = await this.getSessionTree();
    if (tree.leafId && id === String(tree.leafId)) {
      // /clone at tip: omit atTurnId (server tip-fork semantics).
      return undefined;
    }

    const envelopes = await this.listAllItemEnvelopes();
    const env = envelopes.find((row) => String(row.id ?? "") === id);
    const turnId = env ? String(env.turnId ?? env.turn_id ?? "") : "";
    if (turnId) return turnId;
    throw new Error(`Cannot fork: entry ${id} does not map to a user turn`);
  }

  private async listAllItemEnvelopes(): Promise<Array<Record<string, unknown>>> {
    if (!this.sessionId) return [];
    const items: Array<Record<string, unknown>> = [];
    let cursor: string | undefined;
    do {
      const page = (await this.rpc.request("session/items/list", {
        sessionId: this.sessionId,
        ...(cursor ? { cursor } : {}),
        limit: 500,
      })) as { data?: unknown[]; nextCursor?: string | null };
      if (Array.isArray(page?.data)) {
        for (const row of page.data) {
          if (row && typeof row === "object") {
            items.push(row as Record<string, unknown>);
          }
        }
      }
      cursor = page?.nextCursor ?? undefined;
    } while (cursor);
    return items;
  }

  async navigateTree(
    targetId: string,
    options?: {
      summarize?: boolean;
      customInstructions?: string;
      replaceInstructions?: boolean;
      label?: string;
    },
  ) {
    const id = String(targetId ?? "").trim();
    if (!id) return { cancelled: true as const };
    try {
      const result = (await this.rpc.request("session/tree/navigate", {
        sessionId: this.requireSessionId(),
        entryId: id,
        ...(options?.summarize != null ? { summarize: options.summarize } : {}),
        ...(options?.customInstructions != null
          ? { customInstructions: options.customInstructions }
          : {}),
        ...(options?.replaceInstructions != null
          ? { replaceInstructions: options.replaceInstructions }
          : {}),
        ...(options?.label != null ? { label: options.label } : {}),
      })) as {
        leafId?: string | null;
        editorText?: string;
        cancelled?: boolean;
        aborted?: boolean;
      };
      if (result?.cancelled) return { cancelled: true as const, aborted: result.aborted };
      return {
        cancelled: false as const,
        ...(result?.editorText != null ? { editorText: String(result.editorText) } : {}),
        ...(result?.aborted != null ? { aborted: Boolean(result.aborted) } : {}),
      };
    } catch {
      return { cancelled: true as const };
    }
  }

  async importFromJsonl(inputPath: string, cwdOverride?: string) {
    const cwd = cwdOverride ?? this.cwd;
    const resolvedPath = path.isAbsolute(inputPath) ? inputPath : path.resolve(cwd, inputPath);
    if (!fs.existsSync(resolvedPath)) {
      throw new SessionImportFileNotFoundError(resolvedPath);
    }
    try {
      const result = (await this.rpc.request("session/import", {
        path: resolvedPath,
        cwd,
        format: "jsonl",
      })) as { sessionId?: string };
      if (!result?.sessionId) {
        throw new Error("session/import did not return a sessionId");
      }
      await this.switchSession(String(result.sessionId));
      return { cancelled: false };
    } catch (error) {
      if (error instanceof SessionImportFileNotFoundError) throw error;
      const message = error instanceof Error ? error.message : String(error);
      if (/not a file|ENOENT|no such file/i.test(message)) {
        throw new SessionImportFileNotFoundError(resolvedPath);
      }
      throw error instanceof Error ? error : new Error(message);
    }
  }

  async exportToHtml(outputPath?: string) {
    const path = resolveExportPath(outputPath, this.cwd);
    const result = (await this.rpc.request("session/export", {
      sessionId: this.requireSessionId(),
      format: "html",
      ...(path ? { path } : {}),
    })) as { path: string };
    return String(result.path);
  }

  async exportToJsonl(outputPath?: string) {
    const path = resolveExportPath(outputPath, this.cwd);
    const result = (await this.rpc.request("session/export", {
      sessionId: this.requireSessionId(),
      format: "jsonl",
      ...(path ? { path } : {}),
    })) as { path: string };
    return String(result.path);
  }

  async setSessionName(name: string): Promise<void> {
    this.sessionName = name;
    await this.updateSessionMetadata({ title: name });
    this.emitSessionEvent({ type: "session_info_changed", name });
  }

  async getRlmMaxDepthStatus(): Promise<never> {
    throw unsupported("getRlmMaxDepthStatus");
  }

  async setRlmMaxDepth(): Promise<never> {
    throw unsupported("setRlmMaxDepth");
  }

  async renameSavedSession(sessionPath: string, name: string): Promise<void> {
    await this.rpc.request("session/metadata/update", {
      sessionId: sessionPath,
      expectedVersion: 0,
      title: name,
    });
  }

  async deleteSavedSession(sessionPath: string) {
    await this.rpc.request("session/delete", { sessionId: sessionPath });
    return { ok: true as const, method: "unlink" as const };
  }

  async watchSession(activeSessionId: string) {
    const sessionId = String(activeSessionId ?? "").trim();
    if (!sessionId) return undefined;

    const listeners = new Set<AgentConnectionEventListener>();
    let closed = false;
    let subscriptionId: string | null = null;

    try {
      await this.rpc.request("session/resume", { sessionId });
      const sub = (await this.rpc.request("subscription/create", {
        selectors: [{ kind: "session", sessionId }],
        includeSnapshot: false,
        after: [],
      })) as { subscriptionId?: string };
      subscriptionId = sub?.subscriptionId ?? null;
      if (subscriptionId) this.watchedSessionSubs.set(sessionId, subscriptionId);
    } catch {
      return undefined;
    }

    this.sessionWatchers.set(sessionId, listeners);

    return {
      getMessages: async () => {
        try {
          await this.rpc.request("session/resume", { sessionId });
        } catch {
          // best-effort
        }
        const items: unknown[] = [];
        let cursor: string | undefined;
        do {
          const page = (await this.rpc.request("session/items/list", {
            sessionId,
            ...(cursor ? { cursor } : {}),
            limit: 500,
          })) as { data?: unknown[]; nextCursor?: string | null };
          if (Array.isArray(page?.data)) items.push(...page.data);
          cursor = page?.nextCursor ?? undefined;
        } while (cursor);
        return nativeItemsToAgentMessages(items) as never;
      },
      getCommands: () => this.getCommands(),
      subscribe: (listener: AgentConnectionEventListener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
      getToolDefinition: (name: string) => this.getToolDefinition(name),
      close: async () => {
        if (closed) return;
        closed = true;
        listeners.clear();
        this.sessionWatchers.delete(sessionId);
        if (subscriptionId) {
          try {
            await this.rpc.request("subscription/unsubscribe", { subscriptionId });
          } catch {
            // ignore
          }
          this.watchedSessionSubs.delete(sessionId);
        }
      },
    };
  }

  /** Sync a single AuthStorage credential into Native `credential/set`. */
  async setCredentialFromAuth(provider: string, credential: AuthCredentialLike): Promise<unknown> {
    const params = credentialSetParamsFromAuth(provider, credential);
    return this.rpc.request("credential/set", params);
  }

  async listCredentials(): Promise<{ credentials: unknown[] }> {
    return (await this.rpc.request("credential/list", {})) as { credentials: unknown[] };
  }

  async deleteCredential(credentialId: string): Promise<void> {
    await this.rpc.request("credential/delete", { credentialId });
  }

  /**
   * Push all stored AuthStorage credentials to the Native server.
   * Call after `/login` OAuth UI writes local auth.json.
   */
  async syncAuthStorage(authStorage: {
    list: () => string[];
    get: (provider: string) => AuthCredentialLike | undefined;
  }): Promise<void> {
    for (const provider of authStorage.list()) {
      if (provider.startsWith("mcp:")) continue;
      const credential = authStorage.get(provider);
      if (!credential) continue;
      try {
        await this.setCredentialFromAuth(provider, credential);
      } catch {
        // best-effort per provider
      }
    }
  }

  async readPermissionProfile() {
    await this.refreshPermissionProfile();
    return this.permissionProfile;
  }

  async updatePermissionProfile(profile: "default" | "autoReview" | "fullAccess") {
    await this.updateSessionMetadata({
      settings: { permissionProfile: profile },
    });
    this.permissionProfile = profile;
  }

  async dispose(): Promise<void> {
    if (this.disposed) return;
    this.disposed = true;
    this.deferredReverseEmits = [];
    try {
      if (this.subscriptionId) {
        await this.rpc.request("subscription/unsubscribe", {
          subscriptionId: this.subscriptionId,
        });
      }
    } catch {
      // ignore
    }
    for (const [sessionId, subscriptionId] of this.watchedSessionSubs) {
      try {
        await this.rpc.request("subscription/unsubscribe", { subscriptionId });
      } catch {
        // ignore
      }
      this.watchedSessionSubs.delete(sessionId);
    }
    this.sessionWatchers.clear();
    this.subscriptionId = null;
    this.listeners.clear();
    this.rosterListeners.clear();
    this.resolveIdleWaiters();
  }

  /** Public RPC for Native roster / host helpers. */
  requestNative(method: string, params: unknown): Promise<unknown> {
    return this.rpc.request(method, params);
  }

  getSessionVersion(): number {
    return this.sessionVersion;
  }

  /** Subscribe to raw Native JSON-RPC notifications (Agents View roster). */
  onNativeNotification(handler: (method: string, params: unknown) => void): () => void {
    return this.rpc.onNotification(handler);
  }

  private syncActionsFromQueue(): void {
    this.sessionActions = {
      queuedCount: this.queue.steering.length + this.queue.followUp.length,
      steering: [...this.queue.steering],
      followUps: [...this.queue.followUp],
    };
  }
}

function unsupported(name: string): Error {
  return new Error(`unsupported in Devo Native adapter: ${name}`);
}

type ScheduleJobWire = {
  jobId: string;
  kind: "cron" | "heartbeat";
  status: "active" | "paused" | "stopped";
  sessionId: string;
  cwd?: string;
  schedule?: string;
  intervalMs?: number;
  prompt?: string;
  instruction?: string;
  deliveryMode?: "steer" | "followUp";
  label?: string;
  createdAt?: string;
  updatedAt?: string;
  nextRunAt?: string;
  lastRunAt?: string;
  runCount?: number;
};

function toAgentCronJob(job: ScheduleJobWire) {
  const status =
    job.status === "stopped"
      ? ("cancelled" as const)
      : job.status === "paused"
        ? ("paused" as const)
        : ("active" as const);
  const deliveryMode =
    job.deliveryMode === "followUp" ? ("follow_up" as const) : ("steer" as const);
  return {
    id: job.jobId,
    status,
    source: (job.kind === "heartbeat" ? "heartbeat" : "cron") as "cron" | "heartbeat",
    deliveryMode,
    activeSessionId: job.sessionId,
    sessionId: job.sessionId,
    sessionFile: "",
    cwd: job.cwd ?? "",
    label: job.label,
    prompt: job.prompt ?? job.instruction ?? "",
    schedule: {
      kind: (job.intervalMs != null ? "interval" : "cron") as "cron" | "interval",
      expression: job.schedule ?? (job.intervalMs != null ? `every ${job.intervalMs}ms` : ""),
      intervalMs: job.intervalMs,
    },
    createdAt: job.createdAt ?? new Date().toISOString(),
    updatedAt: job.updatedAt ?? new Date().toISOString(),
    nextRunAt: job.nextRunAt,
    lastRunAt: job.lastRunAt,
    runCount: job.runCount ?? 0,
  };
}

function extractText(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((c) => {
      if (!c || typeof c !== "object") return "";
      const part = c as { type?: string; text?: string };
      return part.type === "text" || part.text != null ? String(part.text ?? "") : "";
    })
    .filter(Boolean)
    .join("");
}

export function normalizeAgentModel(model: Record<string, unknown> | null | undefined): AgentConnectionModel | null {
  if (!model || typeof model !== "object") return null;
  const rawId = String(
    model.id ?? model.modelId ?? model.model_id ?? model.model ?? model.slug ?? "",
  );
  // ModelInfo.provider is the wire API; prefer providerId for the vendor key.
  let rawProvider = String(
    model.providerId ?? model.provider_id ?? model.provider ?? "unknown",
  );
  if (!rawId) return null;
  // Session snapshots often send ModelBinding as provider=slug and model=slug
  // (e.g. both "deepseek/deepseek-v4-flash"). Treat slug-shaped providers as
  // vendor/model pairs so catalog matching and /effort levels work.
  if (rawProvider.includes("/") && !looksLikeWireApi(rawProvider)) {
    rawProvider = rawProvider.slice(0, rawProvider.indexOf("/"));
  }
  const slash = rawId.indexOf("/");
  const normalizedId =
    slash >= 0 && model.id == null && model.modelId == null && model.model_id == null
      ? rawId.slice(slash + 1)
      : rawId.includes("/")
        ? rawId.slice(rawId.lastIndexOf("/") + 1)
        : rawId;
  let normalizedProvider = rawProvider;
  if (slash >= 0 && (normalizedProvider === "unknown" || looksLikeWireApi(normalizedProvider))) {
    normalizedProvider = rawId.slice(0, slash);
  } else if (looksLikeWireApi(normalizedProvider) && typeof model.slug === "string") {
    const slugSlash = String(model.slug).indexOf("/");
    if (slugSlash >= 0) normalizedProvider = String(model.slug).slice(0, slugSlash);
  }
  const input = Array.isArray(model.input)
    ? (model.input as string[])
    : modalitiesToInput(model.inputModalities ?? model.input_modalities);
  const capability = model.reasoningCapability ?? model.reasoning_capability;
  const capabilityLevels = Array.isArray((capability as { levels?: unknown } | null)?.levels)
    ? ((capability as { levels: unknown[] }).levels.map(String))
    : [];
  const thinkingLevelMap = coerceThinkingLevelMap(
    model.thinkingLevelMap ?? model.thinking_level_map,
  );
  const reasoning = Boolean(
    model.reasoning ??
      model.supportsReasoning ??
      (capabilityLevels.length > 0 || thinkingLevelMap != null),
  );
  // Match pi-ai getSupportedThinkingLevels: prefer server chip list, else map keys,
  // else capability levels as authored (never invent max↔xhigh renames).
  const availableThinkingLevels = Array.isArray(model.availableThinkingLevels)
    ? (model.availableThinkingLevels as string[])
    : Array.isArray(model.available_thinking_levels)
      ? (model.available_thinking_levels as string[])
      : thinkingLevelMap
        ? supportedThinkingLevelsFromMap(reasoning, thinkingLevelMap)
        : capabilityLevels.length > 0
          ? capabilityLevels
          : undefined;
  return {
    id: normalizedId,
    provider: normalizedProvider,
    name: String(model.name ?? model.displayName ?? model.display_name ?? normalizedId),
    api: (model.api as never) ?? "openai-completions",
    baseUrl: String(model.baseUrl ?? model.base_url ?? ""),
    reasoning,
    input: (input.length > 0 ? input : ["text"]) as never,
    cost: (model.cost as never) ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: Number(model.contextWindow ?? model.context_window ?? 128000),
    maxTokens: Number(model.maxTokens ?? model.max_tokens ?? 8192),
    ...(thinkingLevelMap ? { thinkingLevelMap } : {}),
    ...(availableThinkingLevels && availableThinkingLevels.length > 0
      ? { availableThinkingLevels }
      : {}),
  } as AgentConnectionModel;
}

function findCatalogModel(
  catalog: AgentConnectionModel[],
  needle: { id?: string; provider?: string },
): AgentConnectionModel | undefined {
  const id = String(needle.id ?? "");
  const provider = String(needle.provider ?? "");
  if (!id) return undefined;
  return (
    catalog.find((m) => m.id === id && m.provider === provider) ??
    catalog.find((m) => m.id === id && (!provider || provider.includes("/") || provider === "unknown")) ??
    catalog.find((m) => m.id === id)
  );
}

function preferVendorProvider(current: string | undefined, catalogProvider: string): string {
  if (!current || current === "unknown" || current.includes("/") || looksLikeWireApi(current)) {
    return catalogProvider || current || "unknown";
  }
  return current;
}

const EXTENDED_THINKING_LEVELS: ThinkingLevel[] = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

type ThinkingLevelMap = Record<string, string | null>;

function coerceThinkingLevelMap(value: unknown): ThinkingLevelMap | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const out: ThinkingLevelMap = {};
  for (const [key, mapped] of Object.entries(value as Record<string, unknown>)) {
    if (mapped === null) out[key] = null;
    else if (typeof mapped === "string") out[key] = mapped;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}

/** pi-ai `getSupportedThinkingLevels` — UI chip names are map keys, not wire values. */
function supportedThinkingLevelsFromMap(
  reasoning: boolean,
  map: ThinkingLevelMap | undefined,
): ThinkingLevel[] {
  if (!reasoning) return ["off"];
  return EXTENDED_THINKING_LEVELS.filter((level) => {
    const mapped = map?.[level];
    if (mapped === null) return false;
    if (level === "xhigh" || level === "max") return mapped !== undefined;
    return true;
  });
}

function thinkingLevelMapFromModel(
  model: AgentConnectionModel | null | undefined,
): ThinkingLevelMap | undefined {
  return coerceThinkingLevelMap(
    (model as { thinkingLevelMap?: unknown } | null | undefined)?.thinkingLevelMap,
  );
}

function thinkingLevelsFromModel(model: AgentConnectionModel | null | undefined): ThinkingLevel[] {
  const raw = (model as { availableThinkingLevels?: string[] } | null | undefined)?.availableThinkingLevels;
  if (Array.isArray(raw) && raw.length > 0) {
    return raw.map(String) as ThinkingLevel[];
  }
  const map = thinkingLevelMapFromModel(model);
  if (map) {
    return supportedThinkingLevelsFromMap(Boolean(model?.reasoning), map);
  }
  return [];
}

/** Resolve a persisted/wire selection onto a UI chip (pi clamp + reverse map). */
function normalizeThinkingSelection(
  raw: string,
  levels: ThinkingLevel[],
  map?: ThinkingLevelMap,
): ThinkingLevel {
  const normalized =
    raw === "disabled" || raw === "none" ? "off" : raw.trim().toLowerCase();
  if (levels.length === 0) {
    return (EXTENDED_THINKING_LEVELS.includes(normalized as ThinkingLevel)
      ? normalized
      : "off") as ThinkingLevel;
  }
  if (levels.includes(normalized as ThinkingLevel)) {
    return normalized as ThinkingLevel;
  }
  if (map) {
    for (const level of levels) {
      if (map[level] === normalized) return level;
    }
  }
  return pickDefaultThinkingLevel(levels);
}

function pickDefaultThinkingLevel(levels: ThinkingLevel[]): ThinkingLevel {
  for (const preferred of ["medium", "high", "low", "xhigh", "max", "minimal", "off"] as ThinkingLevel[]) {
    if (levels.includes(preferred)) return preferred;
  }
  return levels[levels.length - 1] ?? "off";
}

function isOsTempExportPath(candidate: string): boolean {
  const tmpRoot = path.resolve(os.tmpdir());
  const resolved = path.resolve(candidate);
  return resolved === tmpRoot || resolved.startsWith(tmpRoot + path.sep);
}

function resolveExportPath(outputPath: string | undefined, cwd: string): string | undefined {
  if (!outputPath) return undefined;
  // Vendored `/share` writes to os.tmpdir(); Native session/export only allows
  // server_home/exports + session cwd. Drop ephemeral temp paths so the server
  // chooses an allowed export location and returns that path for gist upload.
  if (isOsTempExportPath(outputPath)) return undefined;
  return path.isAbsolute(outputPath) ? outputPath : path.resolve(cwd, outputPath);
}

function looksLikeWireApi(value: string): boolean {
  return (
    value.includes("_") ||
    value === "openai-completions" ||
    value === "anthropic-messages" ||
    value.endsWith("messages") ||
    value.endsWith("completions")
  );
}

/** Server ACP permission option id → native approval-scope wire string. */
export function approvalScopeForOptionId(optionId: string): string {
  switch (optionId) {
    case "allow_session":
      return "session";
    case "allow_prefix_rule":
      return "commandPrefixPersist";
    case "allow_path_prefix":
      return "pathPrefix";
    case "allow_path_prefix_persist":
      return "pathPrefixPersist";
    case "allow_host":
      return "host";
    case "allow_host_persist":
      return "hostPersist";
    case "allow_once":
    case "reject_once":
    default:
      return "once";
  }
}

export function reverseRpcToExtensionUiRequest(requestId: string, method: string, params: unknown) {
  const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;
  if (String(method).startsWith("approval/")) {
    // Native reverse requests carry the flat Approval item, whose summary
    // field is `actionSummary`; ACP-shaped params use `summary`/`command`.
    const summary =
      p.actionSummary ??
      p.summary ??
      p.reason ??
      p.command ??
      p.path ??
      p.toolName ??
      p.tool_name ??
      "";
    const detail = typeof summary === "string" && summary ? `\n${summary}` : "";
    const kind = String(method).replace(/^approval\//, "");
    const approvalIdRaw = p.approvalId ?? p.approval_id;
    const approvalId = typeof approvalIdRaw === "string" && approvalIdRaw ? approvalIdRaw : undefined;
    // Offer the server-provided scope choices (once / session / pathPrefix /
    // host / prefix-rule / deny) when present; fall back to a binary confirm.
    const optionRecords = Array.isArray(p.options)
      ? (p.options as Array<Record<string, unknown>>)
      : [];
    if (optionRecords.length > 0) {
      const labels = optionRecords
        .map((option) => (typeof option.name === "string" ? option.name : ""))
        .filter((label) => label.length > 0);
      if (labels.length > 0) {
        return {
          id: requestId,
          method: "select",
          approvalId,
          payload: {
            title: `Approve ${kind}?${detail}`,
            options: labels,
          },
        };
      }
    }
    return {
      id: requestId,
      method: "confirm",
      approvalId,
      payload: {
        title: "Approval required",
        message: `Approve ${kind}?${detail}`,
      },
    };
  }
  if (method === "userInput/request") {
    const questions = (p.questions as Array<Record<string, unknown>>) ?? [];
    const first = questions[0] ?? {};
    const options = Array.isArray(first.options) ? first.options : undefined;
    const requestIdRaw = p.requestId ?? p.request_id;
    const userInputRequestId =
      typeof requestIdRaw === "string" && requestIdRaw ? requestIdRaw : undefined;
    if (options && options.length > 0) {
      return {
        id: requestId,
        method: "select",
        userInputRequestId,
        payload: {
          title: first.question ?? first.header ?? "Question",
          options,
        },
      };
    }
    return {
      id: requestId,
      method: "input",
      userInputRequestId,
      payload: {
        title: first.question ?? first.header ?? "Input required",
        placeholder: first.placeholder ?? "",
      },
    };
  }
  return {
    id: requestId,
    method: "notify",
    payload: { message: `Unhandled host request: ${method}` },
  };
}
