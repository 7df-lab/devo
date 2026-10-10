import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { compileTree } from "./tui";
import sources from "./sources.json";

test("compilation preserves assets and rewrites package exports without shipping TypeScript tests", () => {
  const root = mkdtempSync(join(tmpdir(), "devo-tui-compile-"));
  try {
    mkdirSync(join(root, "src"));
    writeFileSync(join(root, "package.json"), JSON.stringify({ type: "module", main: "./src/index.ts", exports: { ".": "./src/index.ts" }, devDependencies: { tsx: "*" } }));
    writeFileSync(join(root, "src/index.ts"), "export const answer: number = 42;");
    writeFileSync(join(root, "src/index.test.ts"), "throw new Error('must not ship');");
    writeFileSync(join(root, "src/theme.json"), '{"color":"blue"}');
    compileTree(root);
    expect({
      package: JSON.parse(readFileSync(join(root, "package.json"), "utf8")),
      js: existsSync(join(root, "src/index.js")),
      ts: existsSync(join(root, "src/index.ts")),
      test: existsSync(join(root, "src/index.test.js")),
      asset: readFileSync(join(root, "src/theme.json"), "utf8"),
    }).toEqual({ package: { type: "module", main: "./src/index.js", exports: { ".": "./src/index.js" } }, js: true, ts: false, test: false, asset: '{"color":"blue"}' });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("all six runtime targets pin complete SHA256 digests over HTTPS", () => {
  expect(Object.keys(sources.targets).sort()).toEqual([
    "aarch64-apple-darwin", "aarch64-pc-windows-msvc", "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu",
  ]);
  for (const target of Object.values(sources.targets)) {
    for (const source of Object.values(target)) {
      expect({ https: source.url.startsWith("https://"), sha256: /^[a-f0-9]{64}$/.test(source.sha256) }).toEqual({ https: true, sha256: true });
    }
  }
});
