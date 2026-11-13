import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

test("clipboard completions stay scoped to their original draft lifecycle", () => {
  // Isolate module mocks from the rest of the suite. Exercise production
  // InteractiveMode methods while only clipboard I/O and image conversion are mocked.
  const result = spawnSync(process.execPath, [
    "--experimental-test-module-mocks", "--import", "tsx",
    fileURLToPath(new URL("./clipboard-paste-lifecycle.fixture.mjs", import.meta.url)),
  ], { encoding: "utf8", timeout: 30_000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr + result.stdout);
  for (const stage of ["read", "resize"]) for (const scenario of ["normal", "typing", "send", "switch", "fork", "editor-replaced", "stopped", "stash", "restore", "tree-restore", "tree-preserve", "clear", "queue"]) {
    assert.ok(result.stdout.includes(`${stage}/${scenario}: verified`), scenario);
  }
});
