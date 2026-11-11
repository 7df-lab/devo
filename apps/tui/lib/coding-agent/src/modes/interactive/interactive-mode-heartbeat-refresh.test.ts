import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentConnectionHeartbeat } from "../agent-connection/types.js";
import { InteractiveMode } from "./interactive-mode.js";

type Heartbeat = AgentConnectionHeartbeat;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

for (const action of ["pause", "stop"] as const) {
  test(`delayed pre-${action} poll cannot overwrite the authoritative mutation result`, async () => {
    const oldPoll = deferred<Heartbeat[]>();
    const nextPoll = deferred<Heartbeat[]>();
    const mutation = deferred<Heartbeat["job"]>();
    const active = { job: { id: "job-1", activeSessionId: "session-a", source: "heartbeat", status: "active" } } as Heartbeat;
    const updated = { ...active.job, status: action === "pause" ? "paused" : "stopped" } as Heartbeat["job"];
    const expected = action === "pause" ? [{ ...active, job: updated }] : [];
    let calls = 0;
    const published: Heartbeat[][] = [];
    const mode = Object.create(InteractiveMode.prototype) as {
      heartbeatCatalog: Heartbeat[];
      refreshHeartbeatCatalog(): Promise<void>;
      manageHeartbeat(heartbeat: Heartbeat, action: "pause" | "stop"): Promise<void>;
    };
    Object.assign(mode, {
      sessionEventGeneration: 1,
      heartbeatMutationRevision: 0,
      heartbeatCatalog: [active],
      agentConnection: {
        listHeartbeats: () => ++calls === 1 ? oldPoll.promise : nextPoll.promise,
        manageHeartbeat: () => mutation.promise,
      },
      isCurrentConnectionGeneration: () => true,
      applyHeartbeatCatalog: (items: Heartbeat[]) => {
        mode.heartbeatCatalog = items;
        published.push(items);
      },
    });

    const poll = mode.refreshHeartbeatCatalog();
    const change = mode.manageHeartbeat(active, action);
    mutation.resolve(updated);
    await change;
    assert.deepEqual(mode.heartbeatCatalog, expected);
    assert.equal(calls, 1, "post-mutation refresh joins the in-flight lane");

    oldPoll.resolve([active]);
    await Promise.resolve();
    await Promise.resolve();
    assert.equal(calls, 2, "coalesced refresh starts after the older request finishes");
    assert.deepEqual(published, [expected], "stale active snapshot must never be published, even temporarily");

    nextPoll.resolve(expected);
    await poll;
    assert.deepEqual(mode.heartbeatCatalog, expected);
    assert.deepEqual(published, [expected, expected]);
  });
}

for (const lifecycle of ["stopped", "invalidated", "invalidated-then-recovered"] as const) {
  test(`pending heartbeat refresh cannot publish after ${lifecycle}`, async () => {
    const pending = deferred<Heartbeat[]>();
    const published: Heartbeat[][] = [];
    const mode = Object.create(InteractiveMode.prototype) as {
      activityStopped: boolean;
      activityInvalidated: boolean;
      heartbeatMutationRevision: number;
      refreshHeartbeatCatalog(): Promise<void>;
    };
    Object.assign(mode, {
      sessionEventGeneration: 1,
      activityStopped: false,
      activityInvalidated: false,
      heartbeatMutationRevision: 0,
      agentConnection: { listHeartbeats: () => pending.promise },
      isCurrentConnectionGeneration: () => true,
      applyHeartbeatCatalog: (items: Heartbeat[]) => published.push(items),
    });
    const refresh = mode.refreshHeartbeatCatalog();
    if (lifecycle === "stopped") {
      mode.activityStopped = true;
    } else {
      mode.activityInvalidated = true;
      mode.heartbeatMutationRevision++;
      if (lifecycle === "invalidated-then-recovered") mode.activityInvalidated = false;
    }
    pending.resolve([]);
    await refresh;
    assert.deepEqual(published, [], "old pending data cannot render into stopped or invalidated UI");
  });
}
