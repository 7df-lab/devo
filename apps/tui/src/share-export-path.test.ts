import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

test("share uses and cleans the returned export, never a stale temporary file", () => {
  const temp = mkdtempSync(join(tmpdir(), "devo-share-test-"));
  try {
    const result = spawnSync(process.execPath, [
      "--experimental-test-module-mocks", "--import", "tsx",
      fileURLToPath(new URL("./share-export-path.fixture.mjs", import.meta.url)),
    ], { encoding: "utf8", timeout: 30_000, env: { ...process.env, TMPDIR: temp, TMP: temp, TEMP: temp } });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 0, result.stderr + result.stdout);
    for (const scenario of ["success", "upload-failure", "spawn-error", "cancel", "export-failure", "copy-failure", "same-path"]) {
      assert.ok(result.stdout.includes(`${scenario}: verified`));
    }
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
