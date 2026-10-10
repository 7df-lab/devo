import { createHash } from "node:crypto";
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import sources from "./sources.json";
import { buildTui, run } from "./tui";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function download(source: { url: string; sha256: string }, cache: string): Promise<string> {
  mkdirSync(cache, { recursive: true });
  const path = join(cache, source.sha256 + (source.url.endsWith(".zip") ? ".zip" : ".tar.gz"));
  if (!existsSync(path)) {
    run(process.platform === "win32" ? "curl.exe" : "curl", ["-fL", "--retry", "3", source.url, "-o", path + ".partial"]);
    const { renameSync } = await import("node:fs");
    renameSync(path + ".partial", path);
  }
  const actual = createHash("sha256").update(readFileSync(path)).digest("hex");
  if (actual !== source.sha256) {
    rmSync(path);
    throw new Error(`SHA256 mismatch for ${source.url}`);
  }
  return path;
}

export async function bundleRuntime(target: string, output: string, rg: string, binaryDir: string): Promise<void> {
  const runtimeTarget = target.replace("unknown-linux-musl", "unknown-linux-gnu");
  const source = sources.targets[runtimeTarget as keyof typeof sources.targets];
  if (!source) throw new Error(`Unsupported target ${target}`);
  if (existsSync(output)) throw new Error(`Bundle output must be new: ${output}`);
  const platform = target.includes("windows") ? "win32" : target.includes("darwin") ? "darwin" : "linux";
  const arch = target.startsWith("aarch64-") ? "arm64" : "x64";
  const ext = platform === "win32" ? ".exe" : "";
  // Git Bash's GNU tar treats C: paths as remote hosts. Use native bsdtar.
  const tar = process.platform === "win32" ? join(process.env.SystemRoot ?? "C:/Windows", "System32/tar.exe") : "tar";
  for (const file of [join(binaryDir, `devo${ext}`), rg]) {
    if (!existsSync(file)) throw new Error(`Missing release binary ${file}`);
  }
  const work = mkdtempSync(join(tmpdir(), "devo-bundle-"));
  const cache = process.env.DEVO_RUNTIME_CACHE ?? join(tmpdir(), "devo-runtime-cache");
  mkdirSync(join(output, "runtime/node", platform === "win32" ? "." : "bin"), { recursive: true });
  try {
    const nodeArchive = await download(source.node, cache);
    const nodeExtract = join(work, "node");
    mkdirSync(nodeExtract);
    run(tar, ["-xf", nodeArchive, "-C", nodeExtract]);
    const nodeRoot = join(nodeExtract, readdirSync(nodeExtract)[0]);
    const nodePath = join(output, "runtime/node", platform === "win32" ? "node.exe" : "bin/node");
    copyFileSync(join(nodeRoot, platform === "win32" ? "node.exe" : "bin/node"), nodePath);
    copyFileSync(join(nodeRoot, "LICENSE"), join(output, "runtime/node/LICENSE"));
    if (platform !== "win32") chmodSync(nodePath, 0o755);
    const pythonArchive = await download(source.python, cache);
    run(tar, ["-xf", pythonArchive, "-C", join(output, "runtime")]);
    const site = join(output, "runtime/python-site");
    run("uv", ["pip", "install", "--python-version", "3.13", "--python-platform", runtimeTarget,
      "--target", site, "--only-binary", ":all:", "--require-hashes", "--no-compile-bytecode",
      "-r", join(repo, "scripts/runtime/requirements.lock")]);
    copyFileSync(join(repo, "scripts/runtime/sitecustomize.py"), join(site, "sitecustomize.py"));
    cpSync(join(repo, "crates/kernel/rlm-runtime/src/rlm"), join(site, "rlm"), { recursive: true, filter: (path) => basename(path) !== "__pycache__" });
    for (const name of ["agent_observe", "compact", "goal", "refine"]) {
      cpSync(join(repo, "crates/core/rlm_skills", name), join(site, name), { recursive: true, filter: (path) => basename(path) !== "__pycache__" });
    }
    copyFileSync(join(repo, "scripts/runtime/requirements.lock"), join(output, "runtime/requirements.lock"));
    buildTui(repo, join(output, "tui"), work, platform, arch);
    for (const name of ["devo"]) {
      copyFileSync(join(binaryDir, name + ext), join(output, name + ext));
      if (platform !== "win32") chmodSync(join(output, name), 0o755);
    }
    copyFileSync(rg, join(output, "rg" + ext));
    if (platform !== "win32") chmodSync(join(output, "rg"), 0o755);
    for (const name of ["README.md", "LICENSE", "install.sh", "install.ps1"]) copyFileSync(join(repo, name), join(output, name));
    const version = JSON.parse(readFileSync(join(repo, "apps/tui/package.json"), "utf8")).version;
    writeFileSync(join(output, "runtime/manifest.json"), JSON.stringify({
      schema: 1, version, target, runtimeTarget, node: sources.nodeVersion, python: sources.pythonVersion,
      sources: source, pythonRequirementsSha256: createHash("sha256").update(readFileSync(join(repo, "scripts/runtime/requirements.lock"))).digest("hex"),
    }, null, 2) + "\n");
    console.log(`Complete Devo ${version} bundle: ${output}`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const [target, output, rg, binaryDir] = process.argv.slice(2);
  if (!target || !output || !rg || !binaryDir) throw new Error("Usage: bun scripts/runtime/bundle.ts <target> <new-output> <rg> <binary-dir>");
  await bundleRuntime(target, resolve(output), resolve(rg), resolve(binaryDir));
}
