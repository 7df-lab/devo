import assert from "node:assert/strict";
import test from "node:test";
import { subscribeAgentRoster, type NativeRosterClient } from "./native-agents-view-bridge.js";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
const page = (title: string) => ({ data: [{ id: "root", title }], nextCursor: null });

function clientWithDelayedReads() {
  const reads: ReturnType<typeof deferred<unknown>>[] = [];
  let initial = true;
  const client: NativeRosterClient = {
    async request(method) {
      if (method === "session/list") {
        if (initial) { initial = false; return page("initial"); }
        const read = deferred<unknown>();
        reads.push(read);
        return read.promise;
      }
      if (method === "agent/list") return { agents: [] };
      if (method === "subscription/create") return { subscriptionId: "roster" };
      return {};
    },
  };
  return { client, reads };
}

test("roster refresh bursts are serialized with one trailing refresh", async () => {
  const { client, reads } = clientWithDelayedReads();
  let ticks = 0;
  const roster = await subscribeAgentRoster(client, "/repo", () => { ticks++; });
  const first = roster.refresh();
  await flush();
  const second = roster.refresh();
  const third = roster.refresh();
  await flush();
  assert.equal(reads.length, 1, "only one roster query may be in flight");
  reads[0]!.resolve(page("stale"));
  await flush();
  assert.equal(reads.length, 2, "events during a read require one trailing read");
  await Promise.all([first, second, third]);
  reads[1]!.resolve(page("latest"));
  await flush();
  assert.equal(roster.summaries()[0]?.sessionName, "latest");
  assert.equal(ticks, 2, "serialized snapshots continue updating during activity");
  await roster.dispose();
});

test("disposing a roster fences pending results and queued trailing reads", async () => {
  const { client, reads } = clientWithDelayedReads();
  let ticks = 0;
  const roster = await subscribeAgentRoster(client, "/repo", () => { ticks++; });
  const pending = roster.refresh();
  await flush();
  const trailing = roster.refresh();
  await roster.dispose();
  for (const read of reads) read.resolve(page("late"));
  await Promise.all([pending, trailing]);
  assert.equal(roster.summaries()[0]?.sessionName, "initial");
  assert.equal(reads.length, 1);
  assert.equal(ticks, 0);
});


test("roster refresh preserves last good data on failure and accepts later reads", async () => {
  const { client, reads } = clientWithDelayedReads();
  let ticks = 0;
  const roster = await subscribeAgentRoster(client, "/repo", () => { ticks++; });
  const failed = roster.refresh();
  await flush();
  reads[0]!.reject(new Error("temporary transport failure"));
  await failed;
  assert.equal(roster.summaries()[0]?.sessionName, "initial");
  assert.equal(ticks, 0);
  const retry = roster.refresh();
  await flush();
  reads[1]!.resolve(page("recovered"));
  await retry;
  assert.equal(roster.summaries()[0]?.sessionName, "recovered");
  assert.equal(ticks, 1);
  await roster.dispose();
});


test("continuous roster invalidations still publish each read and settle callers", async () => {
  const { client, reads } = clientWithDelayedReads();
  let ticks = 0;
  const roster = await subscribeAgentRoster(client, "/repo", () => { ticks++; });
  let pending = roster.refresh();
  for (let i = 0; i < 3; i++) {
    await flush();
    const invalidation = roster.refresh();
    reads[i]!.resolve(page(`revision-${i}`));
    await Promise.all([pending, invalidation]);
    assert.equal(roster.summaries()[0]?.sessionName, `revision-${i}`);
    assert.equal(ticks, i + 1);
    pending = Promise.resolve();
  }
  await roster.dispose();
  await flush();
  for (const read of reads) read.resolve(page("disposed"));
});

test("roster observes a child created while its subscription is being installed", async () => {
  let handler: ((method: string, params: unknown) => void) | undefined;
  let childExists = false;
  const client: NativeRosterClient = {
    onNotification(callback) { handler = callback; return () => { handler = undefined; }; },
    async request(method) {
      if (method === "subscription/create") {
        childExists = true;
        handler?.("session/created", { session: { id: "child" } });
        return { subscriptionId: "roster" };
      }
      if (method === "session/list") return {
        data: [{ id: "root" }, ...(childExists ? [{ id: "child", parent: { sessionId: "root" }, status: "active" }] : [])],
        nextCursor: null,
      };
      if (method === "agent/list") return { agents: [] };
      return {};
    },
  };
  const roster = await subscribeAgentRoster(client, "/repo", () => {});
  await flush();
  assert.deepEqual(roster.summaries().map(({ sessionId, activity }) => ({ sessionId, activity })), [
    { sessionId: "root", activity: "idle" },
    { sessionId: "child", activity: "working" },
  ]);
  await roster.dispose();
});

test("roster reconciles a child started during the initial snapshot read", async () => {
  const read = deferred<unknown>();
  let handler: ((method: string, params: unknown) => void) | undefined;
  let reads = 0;
  const client: NativeRosterClient = {
    onNotification(callback) { handler = callback; return () => { handler = undefined; }; },
    async request(method) {
      if (method === "subscription/create") return { subscriptionId: "roster" };
      if (method === "session/list") {
        reads++;
        return reads === 1 ? read.promise : { data: [{ id: "child", status: "active" }], nextCursor: null };
      }
      if (method === "agent/list") return { agents: [] };
      return {};
    },
  };
  const pending = subscribeAgentRoster(client, "/repo", () => {});
  await flush();
  handler?.("session/statusChanged", { sessionId: "child", status: "active" });
  read.resolve({ data: [{ id: "child", status: "idle" }], nextCursor: null });
  const roster = await pending;
  await flush();
  assert.equal(roster.summaries()[0]?.activity, "working");
  assert.equal(reads, 2);
  await roster.dispose();
});
