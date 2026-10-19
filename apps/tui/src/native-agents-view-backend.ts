/**
 * Native backend for vendored AgentsViewMode (no vendor daemon).
 */

import type {
  AgentsViewNativeBackend,
  AgentsViewNativeRoster,
} from "@earendil-works/pi-coding-agent/agents-view";
import type { SessionSummary } from "@earendil-works/pi-coding-agent/agents-view";
import { subscribeAgentRoster } from "./native-agents-view-bridge.js";
import type { NativeAgentConnection } from "./native-agent-connection.js";

export function createNativeAgentsViewBackend(options: {
  connection: NativeAgentConnection;
  cwd: string;
}): AgentsViewNativeBackend {
  const { connection, cwd } = options;

  const client = {
    request: (method: string, params: unknown) => connection.requestNative(method, params),
    onNotification: (handler: (method: string, params: unknown) => void) =>
      connection.onNativeNotification(handler),
  };

  return {
    async createRoster(): Promise<AgentsViewNativeRoster> {
      const listeners = new Set<() => void>();
      const notify = () => {
        for (const listener of listeners) {
          try {
            listener();
          } catch {
            // ignore
          }
        }
      };
      const inner = await subscribeAgentRoster(client, cwd, notify);
      return {
        summaries: () => inner.summaries() as SessionSummary[],
        onUpdate: (listener) => {
          listeners.add(listener);
          return () => {
            listeners.delete(listener);
          };
        },
        refresh: () => inner.refresh(),
        dispose: () => inner.dispose(),
      };
    },

    async openSession(summary) {
      const sessionId = summary.sessionId || summary.activeSessionId || summary.id;
      const switched = await connection.switchSession(sessionId, { historyLoadMode: "deferred" });
      if (switched.cancelled) {
        throw new Error(`Failed to resume session ${sessionId}`);
      }
      return {
        connection,
        summary: {
          ...summary,
          activeSessionId: sessionId,
          sessionId,
          isSessionActive: true,
        },
      };
    },

    async createSession() {
      const created = await connection.newSession();
      if (created.cancelled) {
        throw new Error("Failed to create session");
      }
      const state = await connection.getState();
      const sessionId = String(state.sessionId ?? state.activeSessionId ?? "");
      if (!sessionId) throw new Error("session/new returned no session id");
      const summary: SessionSummary = {
        id: sessionId,
        lifecycle: "draft",
        activity: "idle",
        isSessionActive: false,
        runtimeKind: "top-level",
        rlmDepth: 0,
        activeSessionId: sessionId,
        sessionId,
        sessionFile: sessionId,
        sessionName: state.sessionName,
        cwd: state.cwd || cwd,
        model: state.model as SessionSummary["model"],
        thinkingLevel: state.thinkingLevel,
        isStreaming: false,
        isCompacting: false,
        attachedClients: 1,
        messageCount: 0,
        sessionActions: { queuedCount: 0, steering: [], followUps: [] },
      };
      return { connection, summary };
    },

    async renameSession(summary, name) {
      const sessionId = summary.sessionId || summary.id;
      await connection.requestNative("session/metadata/update", {
        sessionId,
        expectedVersion: connection.getSessionVersion(),
        title: name,
      });
    },

    async killSession(summary) {
      const sessionId = summary.sessionId || summary.activeSessionId || summary.id;
      try {
        await connection.requestNative("session/interrupt", {
          scope: { scope: "session", sessionId },
        });
      } catch {
        // Idle sessions may reject interrupt; treat as stopped.
      }
    },

    async deleteSession(summary) {
      const sessionId = summary.sessionId || summary.id;
      const state = await connection.getState();
      const attached = String(state.sessionId ?? state.activeSessionId ?? "");
      await connection.requestNative("session/delete", { sessionId });
      // Deleting the attached session leaves the connection pointing at a
      // tombstone; start a fresh session so the next Agents handoff is clean.
      if (attached && attached === sessionId) {
        try {
          await connection.newSession();
        } catch {
          // Best-effort; Agents View will create/open explicitly next.
        }
      }
    },

    async cancelSubagent(_rootActiveSessionId, childId) {
      await connection.requestNative("agent/cancel", { itemId: childId });
    },

    async getLastAssistantText(activeSessionId) {
      try {
        const page = (await connection.requestNative("session/items/list", {
          sessionId: activeSessionId,
          limit: 100,
        })) as { data?: Array<Record<string, unknown>> };
        const items = Array.isArray(page?.data) ? page.data : [];
        for (let i = items.length - 1; i >= 0; i--) {
          const envelope = items[i]!;
          const item = (envelope.item ?? envelope) as Record<string, unknown>;
          const type = String(item.type ?? "");
          if (type === "assistantMessage" || type === "assistant_message") {
            const text = String(item.text ?? "").trim();
            if (text) return text;
          }
        }
      } catch {
        return undefined;
      }
      return undefined;
    },

    async listHeartbeats() {
      return [];
    },

    async prompt(sessionId, message, streamingBehavior) {
      const switched = await connection.switchSession(sessionId);
      if (switched.cancelled) {
        throw new Error(`Failed to attach session ${sessionId}`);
      }
      await connection.prompt(
        message,
        streamingBehavior === undefined ? {} : { streamingBehavior },
      );
    },
  };
}
