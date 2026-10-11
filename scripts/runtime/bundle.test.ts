import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { bundleRuntime } from "./bundle";
import { WINDOWS_SANDBOX_HELPERS } from "./backend";
import { smokeBundle } from "./smoke";

test("Windows bundles reject either missing sandbox helper before downloads", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo helper bundle QA "));
  try {
    const output = join(root, "bundle");
    writeFileSync(join(root, "devo.exe"), "backend");
    writeFileSync(join(root, "rg.exe"), "ripgrep");
    for (const helper of WINDOWS_SANDBOX_HELPERS) {
      await expect(bundleRuntime("x86_64-pc-windows-msvc", output, join(root, "rg.exe"), root))
        .rejects.toThrow(`Missing release binary ${join(root, helper)}`);
      expect(existsSync(output)).toBe(false);
      writeFileSync(join(root, helper), helper);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("Windows bundle smoke checks require helpers even for cross-target builds", () => {
  const root = mkdtempSync(join(tmpdir(), "devo helper smoke QA "));
  try {
    mkdirSync(join(root, "runtime"));
    writeFileSync(join(root, "runtime/manifest.json"), JSON.stringify({ target: "aarch64-pc-windows-msvc" }));
    for (const helper of WINDOWS_SANDBOX_HELPERS) {
      expect(() => smokeBundle(root)).toThrow(`Incomplete Windows sandbox: ${helper}`);
      writeFileSync(join(root, helper), helper);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});
