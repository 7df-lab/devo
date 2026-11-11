import { test } from "node:test";
import assert from "node:assert/strict";
import { InteractiveMode } from "./interactive-mode.js";

function harness() {
  let starts = 0;
  const mode = Object.create(InteractiveMode.prototype) as {
    activityStopped: boolean;
    connectionState: { sessionId: string | undefined };
    agentConnection: { getState(): Promise<{ sessionId: string }> };
    activityMonitor: { start(): void };
    withActivityTransition<T extends { cancelled: boolean }>(transition: () => Promise<T>): Promise<T>;
  };
  Object.assign(mode, {
    activityStopped: false,
    connectionState: { sessionId: "session-a" },
    agentConnection: { getState: async () => ({ sessionId: "session-a" }) },
    activityMonitor: { start: () => { starts++; } },
  });
  return { mode, starts: () => starts };
}

test("failed and cancelled transitions resume polling the unchanged displayed session", async () => {
  const { mode, starts } = harness();
  const failure = new Error("resume failed");
  await assert.rejects(mode.withActivityTransition(async () => { throw failure; }), (error) => error === failure);
  assert.equal(starts(), 1);
  assert.deepEqual(await mode.withActivityTransition(async () => ({ cancelled: true })), { cancelled: true });
  assert.equal(starts(), 2);
});

test("successful transition leaves polling restart to post-render rebind", async () => {
  const { mode, starts } = harness();
  assert.deepEqual(await mode.withActivityTransition(async () => ({ cancelled: false })), { cancelled: false });
  assert.equal(starts(), 0);
});

test("partial backend switch or changed displayed session cannot restart original polling", async () => {
  for (const changed of ["backend", "display"] as const) {
    const { mode, starts } = harness();
    await mode.withActivityTransition(async () => {
      if (changed === "backend") mode.agentConnection.getState = async () => ({ sessionId: "session-b" });
      else mode.connectionState = { sessionId: "session-b" };
      return { cancelled: true };
    });
    assert.equal(starts(), 0, changed);
  }
});

test("recovery state lookup failure preserves original error and does not restart polling", async () => {
  const { mode, starts } = harness();
  mode.agentConnection.getState = async () => { throw new Error("disconnected"); };
  const failure = new Error("resume failed");
  await assert.rejects(mode.withActivityTransition(async () => { throw failure; }), (error) => error === failure);
  assert.equal(starts(), 0);
});

test("UI stopped during recovery state lookup cannot resurrect polling", async () => {
  const { mode, starts } = harness();
  let resolveState!: (state: { sessionId: string }) => void;
  let enteredLookup!: () => void;
  const lookupStarted = new Promise<void>((resolve) => { enteredLookup = resolve; });
  mode.agentConnection.getState = () => {
    enteredLookup();
    return new Promise((resolve) => { resolveState = resolve; });
  };
  const transition = mode.withActivityTransition(async () => ({ cancelled: true }));
  await lookupStarted;
  mode.activityStopped = true;
  resolveState({ sessionId: "session-a" });
  await transition;
  assert.equal(starts(), 0);
});

test("missing displayed session and failed state lookup do not count as an identity match", async () => {
  const { mode, starts } = harness();
  mode.connectionState = { sessionId: undefined };
  mode.agentConnection.getState = async () => { throw new Error("disconnected"); };
  await mode.withActivityTransition(async () => ({ cancelled: true }));
  assert.equal(starts(), 0);
});
