import assert from "node:assert/strict";
import test from "node:test";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The release bundle deletes apps/tui's `lib/` directory after materializing
 * the file: packages under node_modules (scripts/runtime/tui.ts). A runtime
 * value import that escapes into `../lib/...` therefore only fails in the
 * shipped product, never in the repo checkout. Keep value imports on package
 * specifiers (`@earendil-works/...`); `import type` is erased at transpile and
 * stays fine.
 */
const repoTui = join(dirname(fileURLToPath(import.meta.url)), "..");

function collectSourceFiles(directory: string): string[] {
  const files: string[] = [];
  for (const item of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, item.name);
    if (item.isDirectory()) {
      if (item.name !== "node_modules") files.push(...collectSourceFiles(path));
    } else if (item.name.endsWith(".ts") && !/\.(test|spec|d)\.ts$/.test(item.name)) {
      files.push(path);
    }
  }
  return files;
}

test("runtime sources never value-import from the deleted lib directory", () => {
  const offenders: string[] = [];
  for (const path of collectSourceFiles(join(repoTui, "src"))) {
    const text = readFileSync(path, "utf8");
    for (const match of text.matchAll(/import\s+(type\s+)?([^"]*?)from\s+"(\.\.[/])+lib[/]/g)) {
      const isTypeOnly = Boolean(match[1]) || /^type[\s{]/.test(match[2]);
      if (!isTypeOnly) offenders.push(`${path.replace(repoTui + "/", "")}: ${match[0]}`);
    }
  }
  assert.deepEqual(offenders, []);
});
