import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentConnection } from "../agent-connection/types.js";
import { ActivityMonitor } from "./activity-monitor.js";
const tick = () => new Promise((resolve) => setTimeout(resolve, 15));

test("activity refreshes without keyboard input and retries failures", async () => {
  let count = 0;
  const states: string[] = [];
  const conn = { listHeartbeats: async () => [], listShellTasks: async () => { if (++count === 1) throw new Error("offline"); return []; } } as unknown as AgentConnection;
  const monitor = new ActivityMonitor({ getConnection: () => conn, onHeartbeats: () => {}, onShells: (s) => states.push(s.status), intervalMs: 1 });
  monitor.start();
  await tick();
  monitor.stop();
  assert.equal(states[0], "loading");
  assert.ok(states.includes("error"));
  assert.ok(states.includes("ready"));
});

test("old session and stopped requests never mutate current activity", async () => {
  let resolveOld!: (value: never[]) => void;
  let conn = { listHeartbeats: () => new Promise<never[]>((r) => { resolveOld = r; }) } as unknown as AgentConnection;
  const states: string[] = [];
  const monitor = new ActivityMonitor({ getConnection: () => conn, onHeartbeats: (s) => states.push(s.status), onShells: () => {}, intervalMs: 10000 });
  monitor.start();
  conn = { listHeartbeats: async () => [] } as unknown as AgentConnection;
  monitor.start();
  await tick();
  const expected = [...states];
  resolveOld([]);
  await tick();
  assert.deepEqual(states, expected);
  monitor.stop();
});

test("no shell capability means unavailable, never a fabricated zero", async () => {
  const states: string[] = [];
  const conn = { listHeartbeats: async () => [] } as unknown as AgentConnection;
  const monitor = new ActivityMonitor({ getConnection: () => conn, onHeartbeats: () => {}, onShells: (s) => states.push(s.status) });
  monitor.start(); await tick(); monitor.stop();
  assert.deepEqual(states, ["unavailable"]);
});

for (const pendingSource of ["heartbeats", "shells"] as const) {
  test(`pending ${pendingSource} does not freeze the other source or overlap itself`, async () => {
    let pendingCalls = 0;
    let healthyCalls = 0;
    let resolvePending!: (items: never[]) => void;
    const pending = () => {
      pendingCalls++;
      return new Promise<never[]>((resolve) => { resolvePending = resolve; });
    };
    const healthy = async () => { healthyCalls++; return []; };
    const conn = {
      listHeartbeats: pendingSource === "heartbeats" ? pending : healthy,
      listShellTasks: pendingSource === "shells" ? pending : healthy,
    } as unknown as AgentConnection;
    const states: string[] = [];
    const monitor = new ActivityMonitor({
      getConnection: () => conn,
      onHeartbeats: (snapshot) => states.push(`heartbeats:${snapshot.status}`),
      onShells: (snapshot) => states.push(`shells:${snapshot.status}`),
      intervalMs: 1,
    });
    try {
      monitor.start();
      await tick();
      assert.equal(pendingCalls, 1);
      assert.ok(healthyCalls > 1);
      monitor.stop();
      const stoppedStates = [...states];
      const stoppedCalls = healthyCalls;
      resolvePending([]);
      await tick();
      assert.deepEqual(states, stoppedStates);
      assert.equal(healthyCalls, stoppedCalls);
      assert.equal(pendingCalls, 1);
    } finally {
      monitor.stop();
    }
  });
}
