/**
 * Pure helpers for NativeAgentConnection RPC wiring (goal, queue, resources, tree).
 */

import { randomUUID } from "node:crypto";
import { nativeGoalToAgentGoal, emptyGoalState } from "./native-to-pi/messages.js";
import { DEFAULT_PREVIEW_LIMIT, MAX_PREVIEW_LIMIT } from "./native-traffic-log.js";

export type QueueWireEntry = {
  queueItemId: string;
  position: number;
  preview: string;
  enqueuedAt?: string;
  input?: Array<{ type: string; text?: string }>;
};

export function parseQueueWireEntries(value: unknown): QueueWireEntry[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((entry) => (entry && typeof entry === "object" ? (entry as Record<string, unknown>) : null))
    .filter((entry): entry is Record<string, unknown> => entry != null)
    .map((entry) => ({
      queueItemId: String(entry.queueItemId ?? entry.queue_item_id ?? ""),
      position: Number(entry.position ?? 0),
      preview: String(entry.preview ?? ""),
      enqueuedAt:
        typeof entry.enqueuedAt === "string"
          ? entry.enqueuedAt
          : typeof entry.enqueued_at === "string"
            ? entry.enqueued_at
            : undefined,
      input: Array.isArray(entry.input)
        ? entry.input.map((part) => {
            const record = part && typeof part === "object" ? (part as Record<string, unknown>) : {};
            return {
              type: String(record.type ?? "text"),
              text: typeof record.text === "string" ? record.text : undefined,
            };
          })
        : undefined,
    }))
    .filter((entry) => entry.queueItemId.length > 0)
    .sort((left, right) => left.position - right.position);
}

export function queuePreviewText(entry: QueueWireEntry): string {
  if (entry.preview) return entry.preview;
  const texts = (entry.input ?? [])
    .filter((p) => p.type === "text" && p.text)
    .map((p) => String(p.text));
  return texts.join("\n");
}

export type GoalSlashAction =
  | { kind: "read" }
  | { kind: "transition"; method: string }
  | { kind: "set"; objective: string };

/** Parse `/goal` slash forms. Returns null when the message is not a goal command. */
export function parseGoalSlash(message: string): GoalSlashAction | null {
  const trimmed = message.trim();
  const match = /^\/goal(?:\s+(.*))?$/i.exec(trimmed);
  if (!match) return null;
  const args = (match[1] ?? "").trim();
  if (!args) return { kind: "read" };
  const lower = args.toLowerCase();
  if (lower === "pause") return { kind: "transition", method: "session/goal/pause" };
  if (lower === "resume") return { kind: "transition", method: "session/goal/resume" };
  if (lower === "clear") return { kind: "transition", method: "session/goal/clear" };
  if (lower === "cancel") return { kind: "transition", method: "session/goal/cancel" };
  if (lower === "complete") return { kind: "transition", method: "session/goal/complete" };
  return { kind: "set", objective: args };
}

export type PermissionsSlashAction =
  | { kind: "read" }
  | { kind: "update"; profile: "default" | "autoReview" | "fullAccess" };

export function parsePermissionsSlash(message: string): PermissionsSlashAction | null {
  const trimmed = message.trim();
  const match = /^\/permissions(?:\s+(.*))?$/i.exec(trimmed);
  if (!match) return null;
  const args = (match[1] ?? "").trim();
  if (!args) return { kind: "read" };
  const normalized = args.replace(/[-_\s]/g, "").toLowerCase();
  if (normalized === "default") return { kind: "update", profile: "default" };
  if (normalized === "autoreview" || normalized === "review") {
    return { kind: "update", profile: "autoReview" };
  }
  if (normalized === "fullaccess" || normalized === "full") {
    return { kind: "update", profile: "fullAccess" };
  }
  return null;
}

export type TracesSlashAction =
  | { kind: "status" }
  | { kind: "enable" }
  | { kind: "disable" }
  | { kind: "preview"; limit: number };

/** Parse `/traces` slash forms. Returns null when the message is not a traces command. */
export function parseTracesSlash(message: string): TracesSlashAction | null {
  const trimmed = message.trim();
  const match = /^\/traces(?:\s+(.*))?$/i.exec(trimmed);
  if (!match) return null;
  const args = (match[1] ?? "").trim();
  if (!args) return { kind: "status" };
  const [head, ...rest] = args.split(/\s+/);
  const word = (head ?? "").toLowerCase();
  if (word === "status") return { kind: "status" };
  if (word === "on" || word === "enable") return { kind: "enable" };
  if (word === "off" || word === "disable") return { kind: "disable" };
  if (word === "preview") {
    const raw = rest[0];
    if (raw === undefined) return { kind: "preview", limit: DEFAULT_PREVIEW_LIMIT };
    const requested = Number.parseInt(raw, 10);
    if (!Number.isFinite(requested) || requested <= 0) return null;
    return { kind: "preview", limit: Math.min(requested, MAX_PREVIEW_LIMIT) };
  }
  return null;
}

export function applyGoalFromWire(goal: unknown) {
  return nativeGoalToAgentGoal(goal) as ReturnType<typeof emptyGoalState> & Record<string, unknown>;
}

export type SkillWire = {
  name?: string;
  description?: string;
  path?: string;
  id?: string;
  enabled?: boolean;
  scope?: unknown;
  source?: unknown;
};

export type McpServerWire = {
  name?: string;
  id?: string;
  enabled?: boolean;
  status?: string;
  toolCount?: number;
};

/** Map Native skill scope/source onto InteractiveMode autocomplete tags (#builtin/#user/#project). */
export function mapSkillSourceInfo(skill: {
  path?: unknown;
  id?: unknown;
  name?: unknown;
  scope?: unknown;
  source?: unknown;
}): {
  path: string;
  source: string;
  scope: "user" | "project" | "path";
  origin: "top-level";
} {
  const filePath = String(skill.path ?? skill.id ?? skill.name ?? "");
  const scopeRaw = String(skill.scope ?? "").toLowerCase();
  const sourceRaw =
    skill.source == null
      ? ""
      : typeof skill.source === "string"
        ? skill.source
        : typeof skill.source === "object"
          ? String(Object.keys(skill.source as object)[0] ?? "")
          : String(skill.source);
  const sourceNorm = sourceRaw.toLowerCase();

  if (
    scopeRaw === "system" ||
    scopeRaw === "admin" ||
    sourceNorm === "system" ||
    sourceNorm === "admin" ||
    sourceNorm === "builtin" ||
    sourceNorm === "bundled"
  ) {
    return {
      path: filePath,
      source: "builtin",
      scope: "path",
      origin: "top-level",
    };
  }

  const scope =
    scopeRaw === "repo" || scopeRaw === "project"
      ? ("project" as const)
      : scopeRaw === "user"
        ? ("user" as const)
        : ("path" as const);

  return {
    path: filePath,
    source: sourceNorm || "local",
    scope,
    origin: "top-level",
  };
}

export function mapSkillsToResourceSkills(skills: SkillWire[]) {
  return skills.map((s) => {
    const filePath = String(s.path ?? s.id ?? s.name ?? "");
    return {
      name: String(s.name ?? s.id ?? filePath),
      description: s.description != null ? String(s.description) : undefined,
      filePath,
      sourceInfo: mapSkillSourceInfo(s),
    };
  });
}

export function mapMcpToExtensions(servers: McpServerWire[]) {
  return servers.map((s) => {
    const path = String(s.name ?? s.id ?? "mcp");
    return {
      path,
      sourceInfo: {
        path,
        source: "mcp",
        scope: "user" as const,
        origin: "top-level" as const,
      },
      // Runtime status surfaced through the resource snapshot so the /mcp tab
      // can list configured servers the OAuth catalog does not know about.
      mcp: {
        name: path,
        status: String(s.status ?? "unknown"),
        toolCount: Number(s.toolCount ?? 0),
      },
    };
  });
}

export type TurnWire = {
  id?: string;
  sequence?: number;
  status?: string;
  kind?: string;
  startedAt?: string;
  started_at?: string;
};

export type SessionTreeFlatNode = {
  entry: {
    type: "message";
    id: string;
    parentId: string | null;
    timestamp: string;
    message: { role: "user"; content: Array<{ type: "text"; text: string }> };
  };
  label?: string;
};

/** Build a linear chat-fork tree from session turns (leaf = last turn). */
export function buildSessionTreeFromTurns(
  turns: TurnWire[],
  leafIdHint?: string | null,
): { tree: Array<SessionTreeFlatNode & { children: unknown[] }>; leafId: string | null } {
  const sorted = [...turns].sort((a, b) => Number(a.sequence ?? 0) - Number(b.sequence ?? 0));
  const nodes: Array<SessionTreeFlatNode & { children: Array<SessionTreeFlatNode & { children: unknown[] }> }> =
    [];
  let prevId: string | null = null;
  for (const turn of sorted) {
    const id = String(turn.id ?? "");
    if (!id) continue;
    const timestamp = String(turn.startedAt ?? turn.started_at ?? new Date().toISOString());
    const node = {
      entry: {
        type: "message" as const,
        id,
        parentId: prevId,
        timestamp,
        message: {
          role: "user" as const,
          content: [{ type: "text" as const, text: `Turn ${turn.sequence ?? nodes.length + 1}` }],
        },
      },
      label: `Turn ${turn.sequence ?? nodes.length + 1}`,
      children: [] as Array<SessionTreeFlatNode & { children: unknown[] }>,
    };
    if (prevId == null) {
      nodes.push(node);
    } else {
      const parent = findTreeNode(nodes, prevId);
      if (parent) parent.children.push(node);
      else nodes.push(node);
    }
    prevId = id;
  }
  const leafId =
    leafIdHint && findTreeNode(nodes, leafIdHint)
      ? leafIdHint
      : prevId;
  return { tree: nodes, leafId };
}

function findTreeNode(
  nodes: Array<SessionTreeFlatNode & { children: Array<SessionTreeFlatNode & { children: unknown[] }> }>,
  id: string,
): (SessionTreeFlatNode & { children: Array<SessionTreeFlatNode & { children: unknown[] }> }) | null {
  for (const n of nodes) {
    if (n.entry.id === id) return n;
    const found = findTreeNode(n.children, id);
    if (found) return found;
  }
  return null;
}

export type AuthCredentialLike =
  | { type: "api_key"; key: string }
  | {
      type: "oauth";
      access: string;
      refresh?: string;
      expires?: number;
      accountId?: string;
      enterpriseUrl?: string;
    };

export function credentialSetParamsFromAuth(
  provider: string,
  credential: AuthCredentialLike,
): Record<string, unknown> {
  if (credential.type === "api_key") {
    return {
      provider,
      kind: "api_key",
      secret: credential.key,
    };
  }
  return {
    provider,
    kind: "oauth",
    access: credential.access,
    ...(credential.refresh ? { refresh: credential.refresh } : {}),
    ...(credential.expires != null ? { expiresAt: credential.expires } : {}),
    ...(credential.accountId ? { accountId: credential.accountId } : {}),
    ...(credential.enterpriseUrl ? { enterpriseUrl: credential.enterpriseUrl } : {}),
  };
}

export function countMessageStats(messages: Array<{ role?: string; content?: unknown }>) {
  let userMessages = 0;
  let assistantMessages = 0;
  let toolCalls = 0;
  let toolResults = 0;
  for (const m of messages) {
    if (m.role === "user") userMessages += 1;
    else if (m.role === "assistant") assistantMessages += 1;
    else if (m.role === "toolResult" || m.role === "tool_result") toolResults += 1;
    if (Array.isArray(m.content)) {
      for (const part of m.content) {
        if (part && typeof part === "object" && (part as { type?: string }).type === "toolCall") {
          toolCalls += 1;
        }
      }
    }
  }
  return {
    userMessages,
    assistantMessages,
    toolCalls,
    toolResults,
    totalMessages: messages.length,
  };
}

export function newAgentMessageReceipt(params: {
  targetSessionId: string;
  message: string;
  fromSessionId?: string;
  deliveryStatus: "delivered" | "queued";
}) {
  const now = new Date().toISOString();
  return {
    id: randomUUID(),
    source: "agent_message" as const,
    target: {
      activeSessionId: params.targetSessionId,
      sessionId: params.targetSessionId,
    },
    from: params.fromSessionId
      ? { activeSessionId: params.fromSessionId, sessionId: params.fromSessionId }
      : undefined,
    message: params.message,
    deliveryStatus: params.deliveryStatus,
    ...(params.deliveryStatus === "queued" ? { queuedAt: now } : { deliveredAt: now }),
  };
}

export function workspacePathsFromChanges(result: unknown): string[] {
  if (!result || typeof result !== "object") return [];
  const views = (result as { views?: unknown }).views;
  if (!Array.isArray(views)) return [];
  const paths: string[] = [];
  for (const view of views) {
    if (!view || typeof view !== "object") continue;
    const v = view as Record<string, unknown>;
    const files = v.files ?? v.entries ?? v.changes;
    if (Array.isArray(files)) {
      for (const f of files) {
        if (typeof f === "string") paths.push(f);
        else if (f && typeof f === "object") {
          const path = (f as Record<string, unknown>).path ?? (f as Record<string, unknown>).filePath;
          if (path != null) paths.push(String(path));
        }
      }
    }
    if (typeof v.path === "string") paths.push(v.path);
  }
  return [...new Set(paths)];
}
