import { test } from "node:test";
import assert from "node:assert/strict";
import { nativeShellTasks } from "./shell-activity.js";

test("shell activity projects authoritative session shells, not agents or other sessions", () => {
  const shell = { id: "i1", sessionId: "s1", item: { type: "backgroundTask", taskKind: "shell", state: "completed", command: "ls", cwd: "/repo", exitCode: 0, output: "a" } };
  assert.deepEqual(nativeShellTasks({ tasks: [shell, { ...shell, sessionId: "s2" }, { ...shell, item: { type: "subAgent" } }] }, "s1"), [
    { id: "i1", state: "completed", command: "ls", cwd: "/repo", exitCode: 0, output: "a" },
  ]);
});
test("shell activity puts running tasks first, then lists recent tasks before old ones", () => {
  const shell = (id: string, state: string, createdAt?: string) => ({
    id, createdAt, sessionId: "s", item: { type: "backgroundTask", taskKind: "shell", state },
  });
  const tasks = [
    shell("old-completed", "completed", "2026-01-01T00:00:00Z"),
    shell("older-running", "running", "2026-01-02T00:00:00Z"),
    shell("undated", "failed", "not-a-date"),
    shell("newer-running", "running", "2026-01-03T00:00:00Z"),
    shell("recent-completed", "completed", "2026-01-04T00:00:00Z"),
  ];
  assert.deepEqual(nativeShellTasks({ tasks }, "s").map((task) => task.id), [
    "newer-running", "older-running", "recent-completed", "old-completed", "undated",
  ]);
});

test("shell activity distinguishes broken responses from empty lists", () => {
  assert.deepEqual(nativeShellTasks({ tasks: [] }, "s"), []);
  assert.throws(() => nativeShellTasks({}, "s"), /Invalid/);
});
