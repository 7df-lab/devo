/**
 * Native → Agents View SessionSummary bridge.
 *
 * Roots come from `session/list` (Page.data + nextCursor). Children are nested
 * via `agent/list` fan-out. Live updates use `subscription/create` with
 * `{ kind: "sessionsByCwd", cwd }` plus roster-relevant notifications.
 */

import { measureAsync } from "../lib/coding-agent/src/core/timings.js";

export type NativeRosterClient = {
  request(method: string, params: unknown): Promise<unknown>;
  onNotification?(handler: (method: string, params: unknown) => void): () => void;
};

/** Minimal model stub for Agents View rows (Model<Api>-compatible enough). */
export type AgentsViewModelStub = {
  id: string;
  provider: string;
  name: string;
  api: string;
  reasoning: boolean;
  input: string[];
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number };
  contextWindow: number;
  maxTokens: number;
};

/** Agents View SessionSummary-compatible shape (structural). */
export type AgentsViewSessionSummary = {
  id: string;
  lifecycle: "draft" | "live" | "archived";
  activity: "working" | "idle";
  isSessionActive: boolean;
  runtimeKind?: "top-level" | "subagent";
  rlmDepth?: number;
  activeSessionId?: string;
  sessionId: string;
  sessionFile?: string;
  sessionName?: string;
  cwd: string;
  model?: AgentsViewModelStub;
  thinkingLevel?: string;
  isStreaming: boolean;
  isCompacting: boolean;
  hasRunningRlmChildren?: boolean;
  attachedClients: number;
  messageCount: number;
  sessionActions: {
    queuedCount: number;
    steering: readonly string[];
    followUps: readonly string[];
  };
  created?: string;
  modified?: string;
  firstMessage?: string;
  parentActiveSessionId?: string;
  parentSessionId?: string;
  parentSessionPath?: string;
  rlmChildId?: string;
  summary?: string;
  taskState?: unknown;
  lastActivityAt?: string;
};

function mapNativeModel(model: Record<string, unknown> | undefined): AgentsViewModelStub | undefined {
  if (!model) return undefined;
  const id = String(model.id ?? model.modelId ?? model.model ?? "");
  const provider = String(model.provider ?? "unknown");
  if (!id) return undefined;
  return {
    id,
    provider,
    name: String(model.name ?? model.displayName ?? id),
    api: String(model.api ?? "openai-completions"),
    reasoning: Boolean(model.reasoning ?? model.supportsReasoning),
    input: Array.isArray(model.input) ? model.input.map(String) : ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: Number(model.contextWindow ?? model.context_window ?? 128000),
    maxTokens: Number(model.maxTokens ?? model.max_tokens ?? 8192),
  };
}

const ROSTER_REFRESH_METHODS = new Set([
  "session/statusChanged",
  "session/metadataUpdated",
  "session/created",
  "session/deleted",
  "session/archived",
  "session/closed",
  "agent/started",
  "agent/progress",
  "agent/completed",
  "session/schedule/changed",
]);

const PAGE_LIMIT = 200;

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function parentSessionIdFromNative(session: Record<string, unknown>): string | undefined {
  const parent = asRecord(session.parent);
  if (!parent) return undefined;
  return stringField(parent.sessionId) ?? stringField(parent.session_id);
}

function extractAgentChildIds(agents: unknown[]): Array<{
  itemId?: string;
  agentSessionId: string;
  parentSessionId?: string;
  role?: string;
  task?: string;
  status?: string;
}> {
  const out: Array<{
    itemId?: string;
    agentSessionId: string;
    parentSessionId?: string;
    role?: string;
    task?: string;
    status?: string;
  }> = [];
  for (const entry of agents) {
    const envelope = asRecord(entry);
    if (!envelope) continue;
    const item = asRecord(envelope.item) ?? envelope;
    const type = String(item.type ?? "").toLowerCase();
    if (type !== "subagent" && type !== "sub_agent") continue;
    const agentSessionId =
      stringField(item.agentSessionId) ?? stringField(item.agent_session_id);
    if (!agentSessionId) continue;
    out.push({
      itemId: stringField(envelope.id) ?? stringField(item.id),
      agentSessionId,
      parentSessionId:
        stringField(item.parentSessionId) ?? stringField(item.parent_session_id),
      role: stringField(item.role),
      task: stringField(item.task),
      status: stringField(item.status) ?? stringField(envelope.status),
    });
  }
  return out;
}

export function mapNativeSessionToSummary(
  session: Record<string, unknown>,
  options: {
    fallbackCwd?: string;
    parentSessionId?: string;
    rlmChildId?: string;
    hasRunningRlmChildren?: boolean;
  } = {},
): AgentsViewSessionSummary {
  const id = String(session.id ?? "");
  const cwd = String(session.cwd ?? session.directory ?? options.fallbackCwd ?? "");
  const archived = Boolean(session.archived);
  const status = String(session.status ?? "").toLowerCase();
  const activityRaw = String(session.activity ?? "").toLowerCase();
  const flags = Array.isArray(session.flags) ? session.flags.map(String) : [];
  const working =
    activityRaw === "working" ||
    status === "active" ||
    flags.some((f) =>
      ["compacting", "waitingapproval", "waitinguserinput", "updatinggoal"].includes(
        f.toLowerCase(),
      ),
    );
  const parentSessionId =
    options.parentSessionId ?? parentSessionIdFromNative(session);
  const isSubagent = Boolean(parentSessionId);
  const title = stringField(session.title) ?? stringField(session.name);
  const preview = stringField(session.preview);
  const createdAt = stringField(session.createdAt) ?? stringField(session.created_at);
  const lastActivityAt =
    stringField(session.lastActivityAt) ?? stringField(session.last_activity_at);
  const settings = asRecord(session.settings);
  const reasoningEffort =
    stringField(settings?.reasoningEffort) ?? stringField(settings?.reasoning_effort);
  const model = mapNativeModel(asRecord(session.model));
  const queuedCount = Number(session.queuedCount ?? session.queued_count ?? 0);

  return {
    id,
    lifecycle: archived ? "archived" : "live",
    activity: working ? "working" : "idle",
    isSessionActive: working,
    runtimeKind: isSubagent ? "subagent" : "top-level",
    rlmDepth: isSubagent ? 1 : 0,
    activeSessionId: id,
    sessionId: id,
    // Native has no on-disk session file. Do not put the session id in
    // `sessionFile` — Agents View treats that field as a filesystem path for
    // identity (`file:…`) and saved-session delete. Opaque ids must stay on
    // `sessionId` / `activeSessionId` so identity is `active:…` / `session:…`.
    sessionFile: undefined,
    sessionName: title,
    cwd,
    model,
    thinkingLevel: reasoningEffort,
    isStreaming: status === "active" || working,
    isCompacting: flags.some((f) => f.toLowerCase() === "compacting"),
    hasRunningRlmChildren: options.hasRunningRlmChildren,
    attachedClients: 0,
    messageCount: Number(session.messageCount ?? session.message_count ?? 0),
    sessionActions: {
      queuedCount,
      steering: [],
      followUps: [],
    },
    created: createdAt,
    modified: lastActivityAt ?? createdAt,
    firstMessage: preview,
    parentActiveSessionId: parentSessionId,
    parentSessionId,
    parentSessionPath: parentSessionId,
    rlmChildId: options.rlmChildId,
    summary: stringField(session.summary) ?? preview,
    taskState: stringField(session.taskState) ?? stringField(session.task_state),
    lastActivityAt,
  };
}

export async function listNativeSessionsPaged(
  client: NativeRosterClient,
  options: { cwd?: string; limit?: number } = {},
): Promise<Record<string, unknown>[]> {
  const sessions: Record<string, unknown>[] = [];
  let cursor: string | undefined;
  do {
    const page = (await client.request("session/list", {
      ...(options.cwd ? { cwds: [options.cwd] } : {}),
      includeChildren: true,
      ...(cursor ? { cursor } : {}),
      limit: options.limit ?? PAGE_LIMIT,
    })) as { data?: unknown[]; nextCursor?: string | null };
    for (const row of page?.data ?? []) {
      const session = asRecord(row);
      if (session?.id) sessions.push(session);
    }
    cursor = page?.nextCursor ?? undefined;
  } while (cursor);
  return sessions;
}

async function readSession(
  client: NativeRosterClient,
  sessionId: string,
): Promise<Record<string, unknown> | undefined> {
  try {
    const result = (await client.request("session/read", { sessionId })) as {
      session?: Record<string, unknown>;
    };
    return asRecord(result?.session) ?? asRecord(result);
  } catch {
    return undefined;
  }
}

async function listAgentChildren(
  client: NativeRosterClient,
  sessionId: string,
): Promise<ReturnType<typeof extractAgentChildIds>> {
  try {
    const result = (await client.request("agent/list", { sessionId })) as {
      agents?: unknown[];
    };
    return extractAgentChildIds(result?.agents ?? []);
  } catch {
    return [];
  }
}

/**
 * Fetch root sessions, fan-out `agent/list` (recursively), and map to SessionSummary.
 */
export async function fetchAgentRosterSummaries(
  client: NativeRosterClient,
  options: { cwd?: string } = {},
): Promise<AgentsViewSessionSummary[]> {
  const roots = await listNativeSessionsPaged(client, { cwd: options.cwd });
  const byId = new Map<string, AgentsViewSessionSummary>();
  const childIdsByParent = new Map<string, string[]>();
  const pending: Array<{
    session: Record<string, unknown>;
    parentSessionId?: string;
    rlmChildId?: string;
  }> = roots.map((session) => ({ session }));

  const seen = new Set<string>();
  while (pending.length > 0) {
    const next = pending.shift()!;
    const id = String(next.session.id ?? "");
    if (!id || seen.has(id)) continue;
    seen.add(id);

    const children = await listAgentChildren(client, id);
    const childIds = children.map((c) => c.agentSessionId);
    childIdsByParent.set(id, childIds);

    byId.set(
      id,
      mapNativeSessionToSummary(next.session, {
        fallbackCwd: options.cwd,
        parentSessionId: next.parentSessionId ?? parentSessionIdFromNative(next.session),
        rlmChildId: next.rlmChildId,
        hasRunningRlmChildren: childIds.length > 0,
      }),
    );

    for (const child of children) {
      if (seen.has(child.agentSessionId)) continue;
      const childSession =
        (await readSession(client, child.agentSessionId)) ??
        ({
          id: child.agentSessionId,
          cwd: next.session.cwd ?? options.cwd,
          status: "idle",
          activity: "idle",
          archived: false,
          parent: { kind: "agent", sessionId: id },
          preview: child.task ?? "",
          title: child.role,
          queuedCount: 0,
          flags: [],
        } as Record<string, unknown>);
      pending.push({
        session: childSession,
        parentSessionId: child.parentSessionId ?? id,
        // Prefer Native SubAgent item id so agent/cancel can address the child.
        rlmChildId: child.itemId ?? child.role ?? child.agentSessionId,
      });
    }
  }

  // Recompute hasRunningRlmChildren from mapped child activity.
  for (const [parentId, childIds] of childIdsByParent) {
    const parent = byId.get(parentId);
    if (!parent) continue;
    parent.hasRunningRlmChildren = childIds.some((childId) => {
      const child = byId.get(childId);
      return Boolean(child?.isSessionActive || child?.activity === "working");
    });
  }

  return [...byId.values()];
}

export async function subscribeAgentRoster(
  client: NativeRosterClient,
  cwd: string,
  listener: () => void,
): Promise<{
  summaries(): AgentsViewSessionSummary[];
  refresh(): Promise<void>;
  dispose(): Promise<void>;
}> {
  let current: AgentsViewSessionSummary[] = [];
  let disposed = false;
  let refreshQueued = false;
  let subscriptionId: string | null = null;

  const refresh = async () => {
    if (disposed) return;
    try {
      current = await measureAsync("roster.refresh_summaries", () => fetchAgentRosterSummaries(client, { cwd }));
    } catch {
      // Keep last good roster on transient failures.
    }
    if (!disposed) listener();
  };

  const scheduleRefresh = () => {
    if (disposed || refreshQueued) return;
    refreshQueued = true;
    queueMicrotask(() => {
      refreshQueued = false;
      void refresh();
    });
  };

  current = await measureAsync("roster.initial_summaries", () => fetchAgentRosterSummaries(client, { cwd }));

  try {
    const result = (await client.request("subscription/create", {
      selectors: [{ kind: "sessionsByCwd", cwd }],
      includeSnapshot: false,
    })) as { subscriptionId?: string };
    subscriptionId = result?.subscriptionId ?? null;
  } catch {
    // Poll-free degrade: notification handler still refreshes when connection
    // surfaces roster events (e.g. via NativeAgentConnection.notifyRoster).
  }

  const unsubscribeNotification = client.onNotification?.((method) => {
    if (ROSTER_REFRESH_METHODS.has(method)) scheduleRefresh();
  });

  return {
    summaries: () => current,
    refresh,
    dispose: async () => {
      if (disposed) return;
      disposed = true;
      unsubscribeNotification?.();
      if (subscriptionId) {
        try {
          await client.request("subscription/unsubscribe", { subscriptionId });
        } catch {
          // ignore
        }
      }
      subscriptionId = null;
    },
  };
}
