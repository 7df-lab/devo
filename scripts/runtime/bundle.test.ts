import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bundleRuntime } from "./bundle";
import { WINDOWS_SANDBOX_HELPERS } from "./backend";
import { smokeBundle } from "./smoke";

test("bundled skills have one authored source and no python-site copies", () => {
  const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
  const retiredTrees = ["crates/core/rlm_skills", "crates/skills/src/assets/samples", "crates/server/skills"];
  expect(retiredTrees.filter(path => existsSync(join(repo, path)))).toEqual([]);
  expect(existsSync(join(repo, "crates/skills/assets/bundled/goal/src/goal/__init__.py"))).toBe(true);

  const build = readFileSync(join(repo, "crates/skills/build.rs"), "utf8");
  expect(build).toContain('join("assets/bundled")');
  expect(build).not.toContain('join("samples")');
  const bundle = readFileSync(join(repo, "scripts/runtime/bundle.ts"), "utf8");
  for (const path of [...retiredTrees, "crates/skills/assets/bundled"]) {
    expect(bundle).not.toContain(path);
  }
  expect(bundle).toContain('join(repo, "crates/kernel/rlm-runtime/src/rlm")');
});

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
