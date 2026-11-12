import assert from "node:assert/strict";
import { test } from "node:test";
import { GoalElapsedClock } from "../lib/coding-agent/src/core/goal-elapsed-clock.js";
import { emptyGoalState, type GoalState } from "../lib/coding-agent/src/core/goals.js";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
const goal = (patch: Partial<GoalState> = {}): GoalState => ({
  ...emptyGoalState(), goalId: "one", status: "active", active: true, ...patch,
});
test("live elapsed advances, pauses, resumes and reconciles without double counting", () => {
  let now = 0;
  const clock = new GoalElapsedClock(() => now);
  assert.equal(clock.read(goal(), "s"), 0);
  now = 5000;
  assert.equal(clock.read(goal(), "s"), 5);
  assert.equal(clock.read(goal({ timeUsedSeconds: 5 }), "s"), 5);
  now = 6500;
  assert.equal(clock.read(goal({ status: "paused" }), "s"), 6.5);
  now = 20000;
  assert.equal(clock.read(goal({ status: "paused" }), "s"), 6.5);
  assert.equal(clock.read(goal(), "s"), 6.5);
  now = 21500;
  assert.equal(clock.read(goal(), "s"), 8);
  assert.equal(clock.read(goal({ timeUsedSeconds: 12 }), "s"), 12);
  now = 23000;
  assert.equal(clock.read(goal({ goalId: "two" }), "s"), 0);
  assert.equal(clock.read(goal({ timeUsedSeconds: 4 }), "other-session"), 4);
  assert.equal(clock.read(emptyGoalState(), "other-session"), 0);
});
test("disconnect and restoration exclude offline time and ignore wall timestamps", () => {
  let now = 0;
  const clock = new GoalElapsedClock(() => now);
  assert.equal(clock.read(goal({ timeUsedSeconds: 20, createdAt: -100000, updatedAt: 999999999 }), "s"), 20);
  now = 2000;
  clock.setConnected(false);
  now = 200000;
  assert.equal(clock.read(goal({ timeUsedSeconds: 20 }), "s"), 22);
  clock.setConnected(true);
  assert.equal(clock.read(goal({ timeUsedSeconds: 30 }), "s"), 30);
  now = 201000;
  assert.equal(clock.read(goal({ timeUsedSeconds: 30 }), "s"), 31);
  const reopened = new GoalElapsedClock(() => now);
  assert.equal(reopened.read(goal({ status: "paused", timeUsedSeconds: 30 }), "s"), 30);
  now += 3600000;
  assert.equal(reopened.read(goal({ status: "paused", timeUsedSeconds: 30 }), "s"), 30);
});
test("product tray ticker renders advancing time and stops on pause", () => {
  let now = 0;
  let current = goal();
  let renders = 0;
  const mode = Object.create(InteractiveMode.prototype) as any;
  mode.goalElapsedClock = new GoalElapsedClock(() => now);
  mode.connectionState = { sessionId: "s" };
  mode.getGoalState = () => current;
  mode.subagentSummaryLine = { invalidate() {} };
  mode.ui = { requestRender() { renders++; } };
  mode.syncGoalTray(current);
  try {
    assert.equal(mode.getTrayGoalLabel(), "Pursuing goal (0s)");
    now = 62000;
    mode.goalTrayTimer._onTimeout();
    assert.equal(renders, 1);
    assert.equal(mode.getTrayGoalLabel(), "Pursuing goal (1m 02s)");
    current = goal({ status: "paused" });
    mode.syncGoalTray(current);
    assert.equal(mode.goalTrayTimer, undefined);
    now += 60000;
    assert.equal(mode.getTrayGoalLabel(), "Goal paused (1m 02s)");
  } finally { mode.stopGoalTrayTimer(); }
});
