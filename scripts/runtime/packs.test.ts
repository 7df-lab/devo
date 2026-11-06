import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, statSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { archiveTree, archiveTreeZip, buildPacks } from "./packs";
import { installOnline, writeChecksums } from "./installer-fixture";
import { WINDOWS_SANDBOX_HELPERS } from "./backend";

const target = `${process.arch === "arm64" ? "aarch64" : "x86_64"}-${process.platform === "win32" ? "pc-windows-msvc" : process.platform === "darwin" ? "apple-darwin" : "unknown-linux-musl"}`;

const assetTarget = target.replace("-unknown-linux-", "-linux-");
const WINDOWS_LONG_APP_FILE = `${"long path ".repeat(10)}unicode-路径.txt`;
const WINDOWS_HIDDEN_APP_FILE = ".hidden-unicode-payload";
const WINDOWS_EMPTY_APP_DIR = "empty-app-dir";

function fixture(root: string) {
  const source = join(root, "source");
  const windows = process.platform === "win32";
  const files = [windows ? "devo.exe" : "devo", windows ? "rg.exe" : "rg", windows ? "runtime/node/node.exe" : "runtime/node/bin/node", windows ? "runtime/python/python.exe" : "runtime/python/bin/python3", "runtime/python-site/dill/__init__.py", "runtime/python-site/rlm/repl.py", "tui/src/index.js"];
  if (windows) files.push(...WINDOWS_SANDBOX_HELPERS);
  for (const file of files) {
    mkdirSync(dirname(join(source, file)), { recursive: true });
    writeFileSync(join(source, file), "original " + file, { mode: 0o755 });
  }
  if (windows) {
    writeFileSync(join(source, WINDOWS_LONG_APP_FILE), "unicode payload");
    writeFileSync(join(source, WINDOWS_HIDDEN_APP_FILE), "hidden payload");
    mkdirSync(join(source, WINDOWS_EMPTY_APP_DIR));
  }
  writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify({ schema: 1, version: "0.2.0", target }));
  return source;
}

test.skipIf(process.platform === "win32")("Windows ZIP packs require a Windows build host", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo Windows pack host "));
  try {
    const source = fixture(root);
    writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify({ schema: 1, version: "0.2.0", target: "x86_64-pc-windows-msvc" }));
    await expect(buildPacks(source, join(root, "assets"))).rejects.toThrow("Windows build host");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test.skipIf(process.platform !== "win32")("Windows ZIP rejects reparse points", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo Windows reparse "));
  try {
    const source = fixture(root);
    try {
      symlinkSync(join(source, "runtime", "python"), join(source, "runtime", "python", "linked"), "junction");
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === "EPERM" || code === "EACCES" || code === "ENOTSUP") {
        console.warn(`Skipping Windows reparse test: junction creation unavailable (${code})`);
        return;
      }
      throw error;
    }
    await expect(archiveTreeZip(source, ["runtime/python"], join(root, "out.zip"))).rejects.toThrow("reparse point");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test.skipIf(process.platform !== "win32")("Windows ZIP pack handles zero, one and multiple paths", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo Windows zip path counts "));
  try {
    const source = fixture(root);
    await expect(archiveTreeZip(source, [], join(root, "zero.zip"))).rejects.toThrow("No pack paths");
    await archiveTreeZip(source, ["runtime/python"], join(root, "one.zip"));
    expect(existsSync(join(root, "one.zip"))).toBe(true);
    await archiveTreeZip(source, ["runtime/python", "runtime/node"], join(root, "many.zip"));
    expect(existsSync(join(root, "many.zip"))).toBe(true);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("Linux public pack names omit the vendor while retaining canonical manifest targets", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo Linux asset names "));
  try {
    const source = fixture(root);
    for (const arch of ["x86_64", "aarch64"]) {
      const manifest = { schema: 1, version: "0.2.0", target: `${arch}-unknown-linux-musl` };
      writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify(manifest));
      const output = join(root, arch);
      const index = await buildPacks(source, output);
      const packs = readFileSync(index, "utf8").trim().split("\n").slice(1).map(line => line.split(" "));
      expect({ index: basename(index), assets: packs.map(([kind, digest]) => kind === "app"
        ? `devo-tui-app-v0.2.0-${arch}-linux-musl.tar.gz`
        : `devo-runtime-${kind}-${arch}-linux-musl-${digest}.tar.gz`), manifest: JSON.parse(readFileSync(join(source, "runtime/manifest.json"), "utf8")) })
        .toEqual({ index: `devo-tui-v0.2.0-${arch}-linux-musl.install.txt`, assets: packs.map(parts => parts[2]), manifest });
      expect(readdirSync(output).some(name => name.includes("unknown"))).toBe(false);
    }
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test.skipIf(process.platform === "win32")("runtime tar archives are reproducible and preserve long Unicode paths, executable modes and symlinks", async () => {
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
    // DIAGNOSTIC: gather Windows native tar evidence without hiding exit/stderr.
    const version = Bun.spawnSync([tar, "--version"], { stdout: "pipe", stderr: "pipe" });
    console.log("DIAG tar --version", { exit: version.exitCode, stdout: version.stdout.toString(), stderr: version.stderr.toString() });
    const listed = Bun.spawnSync([tar, "-tzf", first], { stdout: "pipe", stderr: "pipe" });
    console.log("DIAG tar -tzf", { exit: listed.exitCode, stdout: listed.stdout.toString(), stderr: listed.stderr.toString() });
    const result = Bun.spawnSync([tar, "-xzf", first, "-C", extracted], { stdout: "pipe", stderr: "pipe" });
    let extractedEntries: unknown;
    try {
      extractedEntries = readdirSync(extracted, { recursive: true });
    } catch (error) {
      extractedEntries = String(error);
    }
    let longPath: string | null = null;
    try {
      longPath = readFileSync(join(extracted, long), "utf8");
    } catch (error) {
      console.log("DIAG longPath error", String(error));
    }
    console.log("DIAG tar extract", { exit: result.exitCode, stderr: result.stderr.toString(), extractedEntries });
    expect({ exit: result.exitCode, stderr: result.stderr.toString(), longPath }).toEqual({ exit: 0, stderr: "", longPath: "unicode payload" });
    if (process.platform !== "win32") expect(readFileSync(join(extracted, "runtime/python/python-link"), "utf8")).toBe("original runtime/python/bin/python3");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("real installers reuse runtimes across upgrades, repair damage, reject corrupt downloads and serialize cache population", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo shared installer QA "));
  const assets = join(root, "assets");
  const requests: string[] = [];
  const requestLog: Array<{ at: number; name: string }> = [];
  const runtimeUrls: string[] = [];
  let corrupt = "";
  const missing = new Set<string>();
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
    const name = basename(new URL(request.url).pathname);
    requests.push(name);
    requestLog.push({ at: Date.now(), name });
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
    if (process.platform === "win32") {
      const repeat = join(root, "repeat assets");
      const repeatIndex = await buildPacks(source, repeat);
      expect(readFileSync(repeatIndex, "utf8")).toEqual(readFileSync(index, "utf8"));
      const powershell = join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/powershell.exe");
      const listed = Bun.spawnSync([
        powershell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", join(import.meta.dir, "zip-list.ps1"), "-Archive", join(repeat, first[0][2]),
      ], { stdout: "pipe", stderr: "pipe" });
      expect(listed.exitCode).toBe(0);
      expect(listed.stdout.toString().split(/\r?\n/)).toContain(WINDOWS_LONG_APP_FILE);
    }
    writeFileSync(join(source, "runtime/manifest.json"), JSON.stringify({ schema: 1, version: "0.2.1", target }));
    await buildPacks(source, assets);
    const second = readFileSync(join(assets, `devo-tui-v0.2.1-${assetTarget}.install.txt`), "utf8").trim().split("\n").slice(1).map(line => line.split(" "));
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
    expect({ localNode: existsSync(join(destination, "runtime/node")), localPython: existsSync(join(destination, "runtime/python")), nodeReference: existsSync(join(destination, "runtime/node.path")), appModule: existsSync(join(destination, "runtime/python-site/rlm/repl.py")), longUnicode: process.platform === "win32" ? readFileSync(join(destination, WINDOWS_LONG_APP_FILE), "utf8") : "unicode payload", hidden: process.platform === "win32" ? readFileSync(join(destination, WINDOWS_HIDDEN_APP_FILE), "utf8") : "hidden payload", emptyDir: process.platform === "win32" ? statSync(join(destination, WINDOWS_EMPTY_APP_DIR)).isDirectory() && readdirSync(join(destination, WINDOWS_EMPTY_APP_DIR)).length === 0 : true }).toEqual({ localNode: false, localPython: false, nodeReference: true, appModule: true, longUnicode: "unicode payload", hidden: "hidden payload", emptyDir: true });
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
    if (process.platform !== "win32") {
      // A stale lock must fail closed: no automatic deletion. A controlled
      // `sleep` on PATH makes the 300-attempt wait deterministic without a
      // production timeout knob.
      const staleCache = join(root, "stale lock cache");
      mkdirSync(staleCache);
      const staleLock = join(staleCache, `${first[1][1]}.lock`);
      mkdirSync(staleLock);
      writeFileSync(join(staleLock, "pid"), "999999\n");
      const fakeSleepDir = join(root, "fake sleep bin");
      mkdirSync(fakeSleepDir);
      writeFileSync(join(fakeSleepDir, "sleep"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      const blocked = await installOnline(root, join(root, "stale lock install"), staleCache, origin, target, "v0.2.0", "functions", {
        PATH: `${fakeSleepDir}:${process.env.PATH ?? ""}`,
      });
      expect({
        failed: blocked.exit !== 0,
        lockPreserved: existsSync(staleLock),
        pidPreserved: readFileSync(join(staleLock, "pid"), "utf8"),
        manualCleanupMessage: blocked.stderr.includes("remove the stale lock directory manually"),
      }).toEqual({ failed: true, lockPreserved: true, pidPreserved: "999999\n", manualCleanupMessage: true });
      rmSync(staleLock, { recursive: true, force: true });
      const retry = await installOnline(root, join(root, "stale lock retry install"), staleCache, origin, target);
      if (retry.exit !== 0) throw new Error(`${retry.stdout}\n${retry.stderr}`);
      expect(existsSync(staleLock)).toBe(false);
      requests.length = 0;
    }

    requestLog.length = 0;
    const concurrentCache = join(root, "concurrent cache");
    const results = await Promise.all(["one", "two"].map(name => installOnline(root, join(root, name), concurrentCache, origin, target)));
    for (const item of results) if (item.exit !== 0) throw new Error(`${item.stdout}\n${item.stderr}`);
    const concurrentRuntimeRequests = requests.filter(name => name.startsWith("devo-runtime-")).sort();
    if (JSON.stringify(concurrentRuntimeRequests) !== JSON.stringify(first.slice(1).map(parts => parts[2]).sort())) {
      console.log("CONCURRENT_REQUEST_LOG", JSON.stringify(requestLog));
      console.log("CONCURRENT_CACHE", JSON.stringify(readdirSync(concurrentCache, { recursive: true })));
    }
    expect(concurrentRuntimeRequests).toEqual(first.slice(1).map(parts => parts[2]).sort());
    expect(readdirSync(concurrentCache).some(name => name.startsWith(".stage"))).toBe(false);
    const entrypoint = await installOnline(root, join(root, "entrypoint install"), cache, origin, target, "v0.2.0", "entrypoint");
    if (entrypoint.exit !== 0) throw new Error(`${entrypoint.stdout}\n${entrypoint.stderr}`);
    expect(entrypoint.stdout).toContain(process.platform === "win32" ? "Run 'devo'" : "devo is ready");

    const fullName = `devo-tui-v0.2.0-${assetTarget}.${process.platform === "win32" ? "zip" : "tar.gz"}`;
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
    if (assetTarget !== target) {
      // Pinned older Linux releases can still use canonical Rust target names.
      missing.clear();
      const oldIndex = join(assets, `devo-tui-v0.2.0-${target}.install.txt`);
      renameSync(index, oldIndex);
      writeChecksums(assets);
      const oldPacks = await installOnline(root, join(root, "old named packs"), cache, origin, target);
      if (oldPacks.exit !== 0) throw new Error(`${oldPacks.stdout}\n${oldPacks.stderr}`);
      rmSync(oldIndex);
      renameSync(join(assets, fullName), join(assets, fullName.replace(assetTarget, target)));
      writeChecksums(assets);
      requests.length = 0;
      const oldArchive = await installOnline(root, join(root, "old named archive"), cache, origin, target);
      if (oldArchive.exit !== 0) throw new Error(`${oldArchive.stdout}\n${oldArchive.stderr}`);
      expect(requests.includes(fullName.replace(assetTarget, target))).toBe(true);
    }
  } finally { server.stop(true); rmSync(root, { recursive: true, force: true }); }
}, 120_000);

test.skipIf(process.platform !== "win32")("legacy ASCII tar runtime index still installs on Windows", async () => {
  const root = mkdtempSync(join(tmpdir(), "devo legacy tar index "));
  let server: ReturnType<typeof Bun.serve> | undefined;
  try {
    const source = fixture(root);
    rmSync(join(source, WINDOWS_LONG_APP_FILE), { force: true });
    const assets = join(root, "legacy assets");
    mkdirSync(assets);
    const appPaths = readdirSync(source).filter(name => name !== "runtime")
      .concat(readdirSync(join(source, "runtime")).filter(name => !["node", "python", "node.path", "python.path"].includes(name)).map(name => `runtime/${name}`));
    const appName = `devo-tui-app-v0.2.0-${assetTarget}.tar.gz`;
    const nodeName = `devo-runtime-node-${assetTarget}.tar.gz`;
    const pythonName = `devo-runtime-python-${assetTarget}.tar.gz`;
    await archiveTree(source, appPaths, join(assets, appName));
    await archiveTree(source, ["runtime/node"], join(assets, nodeName));
    await archiveTree(source, ["runtime/python"], join(assets, pythonName));
    const digest = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
    const index = join(assets, `devo-tui-v0.2.0-${assetTarget}.install.txt`);
    writeFileSync(index, ["devo-install-v1", `app ${digest(join(assets, appName))} ${appName}`, `node ${digest(join(assets, nodeName))} ${nodeName}`, `python ${digest(join(assets, pythonName))} ${pythonName}`].join("\n") + "\n");
    writeChecksums(assets);
    server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
      const name = basename(new URL(request.url).pathname);
      const file = Bun.file(join(assets, name));
      return await file.exists() ? new Response(file) : new Response("Not found", { status: 404 });
    } });
    const result = await installOnline(root, join(root, "legacy installed"), join(root, "legacy cache"), `http://127.0.0.1:${server.port}`, target);
    if (result.exit !== 0) throw new Error(`${result.stdout}\n${result.stderr}`);
    expect(existsSync(join(root, "legacy installed", "runtime/manifest.json"))).toBe(true);
  } finally { server?.stop(true); rmSync(root, { recursive: true, force: true }); }
}, 30_000);
