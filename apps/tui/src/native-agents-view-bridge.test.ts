/**
 * Trace: L2-DES-RLM-001
 * Verifies: Native session/list Page.data + agent/list fan-out maps to nested
 * Agents View SessionSummary rows (parent pointers, activity, defaults).
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  fetchAgentRosterSummaries,
  mapNativeSessionToSummary,
  subscribeAgentRoster,
  type NativeRosterClient,
} from "./native-agents-view-bridge.js";

function rootSession(overrides: Record<string, unknown> = {}) {
  return {
    id: "ses_root",
    cwd: "/repo",
    status: "idle",
    activity: "idle",
    archived: false,
    title: "Root chat",
    preview: "hello root",
    createdAt: "2026-01-01T00:00:00.000Z",
    lastActivityAt: "2026-01-01T01:00:00.000Z",
    queuedCount: 0,
    flags: [],
    model: { provider: "openai", model: "gpt-test" },
    settings: { reasoningEffort: "medium" },
    ...overrides,
  };
}

function childSession(overrides: Record<string, unknown> = {}) {
  return {
    id: "ses_child",
    cwd: "/repo",
    status: "active",
    activity: "working",
    archived: false,
    title: "Child agent",
    preview: "working on task",
    createdAt: "2026-01-01T00:30:00.000Z",
    lastActivityAt: "2026-01-01T01:30:00.000Z",
    queuedCount: 1,
    flags: [],
    parent: { kind: "agent", sessionId: "ses_root" },
    model: { provider: "openai", model: "gpt-child" },
    ...overrides,
  };
}

function createFakeClient(): NativeRosterClient & {
  requests: Array<{ method: string; params: unknown }>;
  notify: (method: string, params?: unknown) => void;
} {
  const requests: Array<{ method: string; params: unknown }> = [];
  const notificationHandlers = new Set<(method: string, params: unknown) => void>();

  const client: NativeRosterClient & {
    requests: typeof requests;
    notify: (method: string, params?: unknown) => void;
  } = {
    requests,
    notify(method, params = {}) {
      for (const handler of notificationHandlers) handler(method, params);
    },
    onNotification(handler) {
      notificationHandlers.add(handler);
      return () => notificationHandlers.delete(handler);
    },
    async request(method, params) {
      requests.push({ method, params });
      if (method === "session/list") {
        return {
          data: [rootSession()],
          nextCursor: null,
        };
      }
      if (method === "agent/list") {
        const sessionId = (params as { sessionId?: string })?.sessionId;
        if (sessionId === "ses_root") {
          return {
            agents: [
              {
                id: "item_agent_1",
                sessionId: "ses_root",
                item: {
                  type: "subAgent",
                  agentSessionId: "ses_child",
                  parentSessionId: "ses_root",
                  role: "explore",
                  task: "look around",
                  state: "running",
                },
              },
            ],
          };
        }
        return { agents: [] };
      }
      if (method === "session/read") {
        const sessionId = (params as { sessionId?: string })?.sessionId;
        if (sessionId === "ses_child") {
          return { session: childSession() };
        }
        return { session: rootSession() };
      }
      if (method === "subscription/create") {
        return { subscriptionId: "sub_roster_1" };
      }
      if (method === "subscription/unsubscribe") {
        return {};
      }
      return {};
    },
  };
  return client;
}

test("mapNativeSessionToSummary fills SessionSummary defaults and parent pointers", () => {
  const summary = mapNativeSessionToSummary(childSession(), { fallbackCwd: "/repo" });
  assert.deepEqual(summary, {
    id: "ses_child",
    lifecycle: "live",
    activity: "working",
    isSessionActive: true,
    runtimeKind: "subagent",
    rlmDepth: 1,
    activeSessionId: "ses_child",
    sessionId: "ses_child",
    sessionFile: undefined,
    sessionName: "Child agent",
    cwd: "/repo",
    model: {
      id: "gpt-child",
      provider: "openai",
      name: "gpt-child",
      api: "openai-completions",
      reasoning: false,
      input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000,
      maxTokens: 8192,
    },
    thinkingLevel: undefined,
    isStreaming: true,
    isCompacting: false,
    hasRunningRlmChildren: undefined,
    attachedClients: 0,
    messageCount: 0,
    sessionActions: { queuedCount: 1, steering: [], followUps: [] },
    created: "2026-01-01T00:30:00.000Z",
    modified: "2026-01-01T01:30:00.000Z",
    firstMessage: "working on task",
    parentActiveSessionId: "ses_root",
    parentSessionId: "ses_root",
    parentSessionPath: "ses_root",
    rlmChildId: undefined,
    summary: "working on task",
    taskState: undefined,
    lastActivityAt: "2026-01-01T01:30:00.000Z",
  });
});

test("fetchAgentRosterSummaries nests agent/list children under session/list roots", async () => {
  const client = createFakeClient();
  const summaries = await fetchAgentRosterSummaries(client, { cwd: "/repo" });

  assert.equal(summaries.length, 2);
  const root = summaries.find((s) => s.sessionId === "ses_root");
  const child = summaries.find((s) => s.sessionId === "ses_child");
  assert.ok(root);
  assert.ok(child);
  assert.equal(root.runtimeKind, "top-level");
  assert.equal(root.sessionName, "Root chat");
  assert.equal(root.hasRunningRlmChildren, true);
  assert.equal(child.runtimeKind, "subagent");
  assert.equal(child.parentSessionId, "ses_root");
  assert.equal(child.rlmChildId, "item_agent_1");
  assert.equal(child.activity, "working");

  const listCalls = client.requests.filter((r) => r.method === "session/list");
  assert.equal(listCalls.length, 1);
  assert.deepEqual(listCalls[0]?.params, {
    cwds: ["/repo"],
    includeChildren: true,
    limit: 200,
  });

  const agentListParents = client.requests
    .filter((r) => r.method === "agent/list")
    .map((r) => (r.params as { sessionId: string }).sessionId)
    .sort();
  assert.deepEqual(agentListParents, ["ses_child", "ses_root"]);
});

test("subscribeAgentRoster creates SessionsByCwd selector and refreshes on notifications", async () => {
  const client = createFakeClient();
  let ticks = 0;
  const roster = await subscribeAgentRoster(client, "/repo", () => {
    ticks += 1;
  });

  const create = client.requests.find((r) => r.method === "subscription/create");
  assert.deepEqual(create?.params, {
    selectors: [{ kind: "sessionsByCwd", cwd: "/repo" }],
    includeSnapshot: false,
  });

  assert.equal(roster.summaries().length, 2);
  const before = ticks;
  client.notify("session/statusChanged", { sessionId: "ses_root", status: "active" });
  await new Promise((resolve) => queueMicrotask(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(ticks > before);

  await roster.dispose();
  const unsub = client.requests.find((r) => r.method === "subscription/unsubscribe");
  assert.deepEqual(unsub?.params, { subscriptionId: "sub_roster_1" });
});
