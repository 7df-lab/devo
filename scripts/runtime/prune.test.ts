import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { pruneNodeInputs } from "./prune";

test("pruning preserves runtime assets and source-only modules", () => {
  const root = mkdtempSync(join(tmpdir(), "devo prune "));
  const removed = ["sdk/index.d.ts", "sdk/index.d.mts", "sdk/index.js.map", "sdk/index.ts"];
  const kept = ["sdk/index.js", "sdk/package.json", "sdk/LICENSE.md", "sdk/templates/data.json", "addon/binding.node", "addon/image.wasm", "extension/source-only.ts"];
  try {
    for (const file of [...removed, ...kept]) {
      mkdirSync(dirname(join(root, file)), { recursive: true });
      writeFileSync(join(root, file), "fixture");
    }
    pruneNodeInputs(root);
    expect(removed.map((file) => existsSync(join(root, file)))).toEqual(removed.map(() => false));
    expect(kept.map((file) => existsSync(join(root, file)))).toEqual(kept.map(() => true));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
