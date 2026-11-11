import { test } from "node:test";
import assert from "node:assert/strict";
import { nativeShellTasks } from "./shell-activity.js";

test("shell activity projects authoritative session shells, not agents or other sessions", () => {
  const shell = { id: "i1", sessionId: "s1", item: { type: "backgroundTask", taskKind: "shell", state: "completed", command: "ls", cwd: "/repo", exitCode: 0, output: "a" } };
  assert.deepEqual(nativeShellTasks({ tasks: [shell, { ...shell, sessionId: "s2" }, { ...shell, item: { type: "subAgent" } }] }, "s1"), [
    { id: "i1", state: "completed", command: "ls", cwd: "/repo", exitCode: 0, output: "a" },
  ]);
});
test("shell activity distinguishes broken responses from empty lists", () => {
  assert.deepEqual(nativeShellTasks({ tasks: [] }, "s"), []);
  assert.throws(() => nativeShellTasks({}, "s"), /Invalid/);
});
