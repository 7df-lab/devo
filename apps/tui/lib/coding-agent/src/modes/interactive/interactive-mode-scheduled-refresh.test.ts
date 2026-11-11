import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentConnectionScheduledTask } from "../agent-connection/types.js";
import { InteractiveMode } from "./interactive-mode.js";

type Scheduled = AgentConnectionScheduledTask;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

for (const action of ["pause", "stop"] as const) {
  test(`delayed pre-${action} poll cannot overwrite the authoritative mutation result`, async () => {
    const oldPoll = deferred<Scheduled[]>();
    const nextPoll = deferred<Scheduled[]>();
    const mutation = deferred<Scheduled["job"]>();
    const active = { job: { id: "job-1", activeSessionId: "session-a", source: "heartbeat", status: "active" } } as Scheduled;
    const updated = { ...active.job, status: action === "pause" ? "paused" : "stopped" } as Scheduled["job"];
    const expected = action === "pause" ? [{ ...active, job: updated }] : [];
    let calls = 0;
    const published: Scheduled[][] = [];
    const mode = Object.create(InteractiveMode.prototype) as {
      scheduledCatalog: Scheduled[];
      refreshScheduledCatalog(): Promise<void>;
      manageScheduledTask(scheduled: Scheduled, action: "pause" | "stop"): Promise<void>;
    };
    Object.assign(mode, {
      sessionEventGeneration: 1,
      scheduledMutationRevision: 0,
      scheduledCatalog: [active],
      agentConnection: {
        listScheduledTasks: () => ++calls === 1 ? oldPoll.promise : nextPoll.promise,
        manageScheduledTask: () => mutation.promise,
      },
      isCurrentConnectionGeneration: () => true,
      applyScheduledCatalog: (items: Scheduled[]) => {
        mode.scheduledCatalog = items;
        published.push(items);
      },
    });

    const poll = mode.refreshScheduledCatalog();
    const change = mode.manageScheduledTask(active, action);
    mutation.resolve(updated);
    await change;
    assert.deepEqual(mode.scheduledCatalog, expected);
    assert.equal(calls, 1, "post-mutation refresh joins the in-flight lane");

    oldPoll.resolve([active]);
    await Promise.resolve();
    await Promise.resolve();
    assert.equal(calls, 2, "coalesced refresh starts after the older request finishes");
    assert.deepEqual(published, [expected], "stale active snapshot must never be published, even temporarily");

    nextPoll.resolve(expected);
    await poll;
    assert.deepEqual(mode.scheduledCatalog, expected);
    assert.deepEqual(published, [expected, expected]);
  });
}

for (const lifecycle of ["stopped", "invalidated", "invalidated-then-recovered"] as const) {
  test(`pending heartbeat refresh cannot publish after ${lifecycle}`, async () => {
    const pending = deferred<Scheduled[]>();
    const published: Scheduled[][] = [];
    const mode = Object.create(InteractiveMode.prototype) as {
      activityStopped: boolean;
      activityInvalidated: boolean;
      scheduledMutationRevision: number;
      refreshScheduledCatalog(): Promise<void>;
    };
    Object.assign(mode, {
      sessionEventGeneration: 1,
      activityStopped: false,
      activityInvalidated: false,
      scheduledMutationRevision: 0,
      agentConnection: { listScheduledTasks: () => pending.promise },
      isCurrentConnectionGeneration: () => true,
      applyScheduledCatalog: (items: Scheduled[]) => published.push(items),
    });
    const refresh = mode.refreshScheduledCatalog();
    if (lifecycle === "stopped") {
      mode.activityStopped = true;
    } else {
      mode.activityInvalidated = true;
      mode.scheduledMutationRevision++;
      if (lifecycle === "invalidated-then-recovered") mode.activityInvalidated = false;
    }
    pending.resolve([]);
    await refresh;
    assert.deepEqual(published, [], "old pending data cannot render into stopped or invalidated UI");
  });
}
