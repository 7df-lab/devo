import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { archiveTree, buildPacks } from "./packs";
import { installOnline, writeChecksums } from "./installer-fixture";

const target = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-${process.platform === "win32" ? "pc-windows-msvc" : process.platform === "darwin" ? "apple-darwin" : "unknown-linux-musl"}`;

function fixture(root: string) {
  const source = join(root, "source");
  const windows = process.platform === "win32";
  const files = [windows ? "devo.exe" : "devo", windows ? "rg.exe" : "rg", windows ? "runtime/node/node.exe" : "runtime/node/bin/node", windows ? "runtime/python/python.exe" : "runtime/python/bin/python3", "runtime/python-site/dill/__init__.py", "runtime/python-site/rlm/repl.py", "tui/src/index.js"];
  for (const file of files) {
    mkdirSync(dirname(join(source, file)), { recursive: true });
    writeFileSync(join(source, file), "original " + file, { mode: 0o755 });
  }
  writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify({ schema: 1, version: "0.2.0", target }));
  return source;
}

test("runtime tar archives are reproducible and preserve long Unicode paths, executable modes and symlinks", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo tar QA "));
  try {
    const source = fixture(root);
    const long = `runtime/python/${"long path ".repeat(14)}unicode-路径.txt`;
    writeFileSync(join(source, long), "unicode payload");
    if (process.platform !== "win32") symlinkSync("bin/python3", join(source, "runtime/python/python-link"));
    const first = join(root, "first.tar.gz");
    const second = join(root, "second.tar.gz");
    await archiveTree(source, ["runtime/python", "runtime/node"], first);
    utimesSync(join(source, "runtime/node"), new Date(), new Date());
    await archiveTree(source, ["runtime/node", "runtime/python"], second);
    expect(readFileSync(second)).toEqual(readFileSync(first));
    const extracted = join(root, "extracted");
    mkdirSync(extracted);
    const tar = process.platform === "win32" ? join(process.env.SystemRoot!, "System32/tar.exe") : "tar";
    const result = Bun.spawnSync([tar, "-xzf", first, "-C", extracted], { stdout: "pipe", stderr: "pipe" });
    expect({ exit: result.exitCode, stderr: result.stderr.toString(), longPath: readFileSync(join(extracted, long), "utf8") }).toEqual({ exit: 0, stderr: "", longPath: "unicode payload" });
    if (process.platform !== "win32") expect(readFileSync(join(extracted, "runtime/python/python-link"), "utf8")).toBe("original runtime/python/bin/python3");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("real installers reuse runtimes across upgrades, repair damage, reject corrupt downloads and serialize cache population", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo shared installer QA "));
  const assets = join(root, "assets");
  const requests: string[] = [];
  const runtimeUrls: string[] = [];
  let corrupt = "";
  const missing = new Set<string>();
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
    const name = basename(new URL(request.url).pathname);
    requests.push(name);
    if (name.startsWith("devo-runtime-")) runtimeUrls.push(new URL(request.url).pathname);
    if (missing.has(name)) return new Response("Not found", { status: 404 });
    if (name === corrupt) return new Response("corrupt bytes");
    const file = Bun.file(join(assets, name));
    return await file.exists() ? new Response(file) : new Response("Not found", { status: 404 });
  } });
  try {
    const source = fixture(root);
    const index = await buildPacks(source, assets);
    const first = readFileSync(index, "utf8").trim().split("\n").slice(1).map(line => line.split(" "));
    expect(first.map(parts => parts[0])).toEqual(["app", "node", "python"]);
    writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify({ schema: 1, version: "0.2.1", target }));
    await buildPacks(source, assets);
    const second = readFileSync(join(assets, `devo-tui-v0.2.1-${target}.install.txt`), "utf8").trim().split("\n").slice(1).map(line => line.split(" "));
    expect(second.slice(1)).toEqual(first.slice(1));
    expect(second[0][1]).not.toBe(first[0][1]);
    writeChecksums(assets);
    const cache = join(root, "cache with spaces");
    const destination = join(root, "installed with spaces");
    const origin = `http://127.0.0.1:${server.port}`;
    let result = await installOnline(root, destination, cache, origin, target);
    if (result.exit !== 0) throw new Error(`${result.stdout}\n${result.stderr}`);
    expect(requests.filter(name => name.startsWith("devo-runtime-")).sort()).toEqual(first.slice(1).map(parts => parts[2]).sort());
    expect(runtimeUrls.every(path => path.startsWith("/runtime-packs/"))).toBe(true);
    expect({ localNode: existsSync(join(destination, "runtime/node")), localPython: existsSync(join(destination, "runtime/python")), nodeReference: existsSync(join(destination, "runtime/node.path")), appModule: existsSync(join(destination, "runtime/python-site/rlm/repl.py")) }).toEqual({ localNode: false, localPython: false, nodeReference: true, appModule: true });
    requests.length = 0;
    result = await installOnline(root, destination, cache, origin, target, "v0.2.1");
    if (result.exit !== 0) throw new Error(`${result.stdout}\n${result.stderr}`);
    expect({ version: JSON.parse(readFileSync(join(destination, "runtime/manifest.json"), "utf8")).version, runtimeDownloads: requests.filter(name => name.startsWith("devo-runtime-")) }).toEqual({ version: "0.2.1", runtimeDownloads: [] });
    const nodeRoot = readFileSync(join(destination, "runtime/node.path"), "utf8").trimEnd();
    writeFileSync(join(nodeRoot, process.platform === "win32" ? "node.exe" : "bin/node"), "damaged");
    requests.length = 0;
    result = await installOnline(root, destination, cache, origin, target, "v0.2.1");
    if (result.exit !== 0) throw new Error(`${result.stdout}\n${result.stderr}`);
    expect(requests.filter(name => name.startsWith("devo-runtime-"))).toEqual([first[1][2]]);

    // A valid index with a corrupted pack must never fall back to a larger
    // archive or replace the existing app, and no cache entry is promoted.
    corrupt = first[1][2];
    const corruptCache = join(root, "corrupt cache");
    result = await installOnline(root, destination, corruptCache, origin, target);
    expect({ failed: result.exit !== 0, existingVersion: JSON.parse(readFileSync(join(destination, "runtime/manifest.json"), "utf8")).version, promotedCorruption: existsSync(join(corruptCache, first[1][1], ".complete")) }).toEqual({ failed: true, existingVersion: "0.2.1", promotedCorruption: false });
    corrupt = "";
    // Missing packs and a corrupted index are errors, never a request to fall
    // back to a complete archive. Only an index HTTP 404 selects old releases.
    missing.add(first[1][2]);
    requests.length = 0;
    const unavailable = await installOnline(root, destination, join(root, "unavailable cache"), origin, target);
    expect({ failed: unavailable.exit !== 0, requestedFullArchive: requests.some(name => /^devo-tui-v.*\.(zip|tar\.gz)$/.test(name)) }).toEqual({ failed: true, requestedFullArchive: false });
    missing.clear();
    corrupt = basename(index);
    const corruptIndex = await installOnline(root, destination, cache, origin, target);
    expect({ failed: corruptIndex.exit !== 0, version: JSON.parse(readFileSync(join(destination, "runtime/manifest.json"), "utf8")).version }).toEqual({ failed: true, version: "0.2.1" });
    corrupt = "";
    requests.length = 0;
    const concurrentCache = join(root, "concurrent cache");
    const results = await Promise.all(["one", "two"].map(name => installOnline(root, join(root, name), concurrentCache, origin, target)));
    for (const item of results) if (item.exit !== 0) throw new Error(`${item.stdout}\n${item.stderr}`);
    expect(requests.filter(name => name.startsWith("devo-runtime-")).sort()).toEqual(first.slice(1).map(parts => parts[2]).sort());
    expect(readdirSync(concurrentCache).some(name => name.startsWith(".stage"))).toBe(false);
    const entrypoint = await installOnline(root, join(root, "entrypoint install"), cache, origin, target, "v0.2.0", "entrypoint");
    if (entrypoint.exit !== 0) throw new Error(`${entrypoint.stdout}\n${entrypoint.stderr}`);
    expect(entrypoint.stdout).toContain(process.platform === "win32" ? "Run 'devo'" : "devo is ready");

    const fullName = `devo-tui-v0.2.0-${target}.${process.platform === "win32" ? "zip" : "tar.gz"}`;
    if (process.platform === "win32") {
      const path = join(root, "legacy.ps1");
      const quote = (value: string) => `'${value.replaceAll("'", "''")}'`;
      writeFileSync(path, `Compress-Archive -Path ${quote(join(source, "*"))} -DestinationPath ${quote(join(assets, fullName))}`);
      const result = Bun.spawnSync([join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/powershell.exe"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path], { stdout: "pipe", stderr: "pipe", env: { ...process.env, PSModulePath: join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/Modules") } });
      expect({ exit: result.exitCode, stderr: result.stderr.toString() }).toEqual({ exit: 0, stderr: "" });
    } else {
      await archiveTree(source, readdirSync(source), join(assets, fullName));
    }
    writeChecksums(assets);
    missing.add(basename(index));
    requests.length = 0;
    const legacyDestination = join(root, "older release");
    const legacy = await installOnline(root, legacyDestination, join(root, "legacy cache"), origin, target);
    if (legacy.exit !== 0) throw new Error(`${legacy.stdout}\n${legacy.stderr}`);
    expect({ fullArchive: requests.includes(fullName), localNode: existsSync(join(legacyDestination, "runtime/node")), reference: existsSync(join(legacyDestination, "runtime/node.path")) }).toEqual({ fullArchive: true, localNode: true, reference: false });
  } finally { server.stop(true); rmSync(root, { recursive: true, force: true }); }
}, 120_000);
