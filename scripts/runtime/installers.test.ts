import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const repo = resolve(import.meta.dir, "../..");

test.skipIf(process.platform !== "win32")("Windows installation repairs an incomplete tree and rolls back a locked sidecar", () => {
  const root = mkdtempSync(join(tmpdir(), "devo installer test "));
  try {
    const source = join(root, "source");
    const destination = join(root, "installed");
    const files = ["devo.exe", "rg.exe", "runtime/node/node.exe", "runtime/python/python.exe", "runtime/python-site/dill/__init__.py", "runtime/python-site/rlm/repl.py", "tui/src/index.js"];
    for (const file of files) {
      mkdirSync(dirname(join(source, file)), { recursive: true });
      writeFileSync(join(source, file), "new");
    }
    writeFileSync(join(source, "runtime/manifest.json"), '{"schema":1}');
    mkdirSync(destination);
    writeFileSync(join(destination, "devo.exe"), "old backend only");
    const quote = (path: string) => `'${path.replace(/'/g, "''")}'`;
    const script = readFileSync(join(repo, "install.ps1"), "utf8").replace(/\nMain\s*$/, "\n") + `
      Install-DevoBundle -Source ${quote(source)} -InstallDir ${quote(destination)}
      $complete = Test-DevoBundle -Directory ${quote(destination)}
      $assets = ${quote(join(root, "assets"))}
      New-Item -ItemType Directory -Path $assets | Out-Null
      $archive = Join-Path $assets "devo-tui-v0.2.0-$(Get-Target).zip"
      Compress-Archive -Path ${quote(join(source, "*"))} -DestinationPath $archive
      Set-Content -LiteralPath (Join-Path $assets 'SHA256SUMS.txt') -Value "$((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLower())  $([IO.Path]::GetFileName($archive))"
      Install-DevoOffline -AssetDir $assets -InstallDir ${quote(join(root, "offline installed"))} -TempRoot ${quote(join(root, "extracted"))} 6>$null
      $offlineComplete = Test-DevoBundle -Directory ${quote(join(root, "offline installed"))}
      $rejected = $false
      try { Install-DevoBundle -Source ${quote(root)} -InstallDir ${quote(destination)} } catch { $rejected = $true }
      Set-Content -LiteralPath ${quote(join(destination, "runtime/manifest.json"))} -Value 'old runtime'
      $lock = [IO.File]::Open(${quote(join(destination, "rg.exe"))}, 'Open', 'ReadWrite', 'None')
      $failed = $false
      try { Install-DevoBundle -Source ${quote(source)} -InstallDir ${quote(destination)} } catch { $failed = $true } finally { $lock.Dispose() }
      $restored = (Get-Content -LiteralPath ${quote(join(destination, "runtime/manifest.json"))} -Raw).Trim() -eq 'old runtime'
      @{complete=$complete; offlineComplete=$offlineComplete; rejected=$rejected; rolledBack=($failed -and $restored)} | ConvertTo-Json -Compress
    `;
    const path = join(root, "test.ps1");
    writeFileSync(path, script);
    const result = Bun.spawnSync([join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/powershell.exe"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path], {
      stdout: "pipe", stderr: "pipe",
      env: { ...process.env, PSModulePath: join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/Modules") },
    });
    if (result.exitCode !== 0) throw new Error(result.stderr.toString());
    expect({ exit: result.exitCode, stderr: result.stderr.toString(), checks: JSON.parse(result.stdout.toString()) }).toEqual({ exit: 0, stderr: "", checks: { complete: true, offlineComplete: true, rejected: true, rolledBack: true } });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 60_000);

test.skipIf(process.platform === "win32")("Unix offline installation copies private runtimes and repairs the same version", () => {
  const root = mkdtempSync(join(tmpdir(), "devo installer test "));
  try {
    const source = join(root, "source");
    const destination = join(root, "installed");
    const files = ["devo", "rg", "runtime/node/bin/node", "runtime/python/bin/python3", "runtime/python-site/dill/__init__.py", "runtime/python-site/rlm/repl.py", "runtime/manifest.json", "tui/src/index.js"];
    for (const file of files) {
      mkdirSync(dirname(join(source, file)), { recursive: true });
      writeFileSync(join(source, file), file === "devo" ? "#!/bin/sh\necho devo 0.2.0\n" : "new", { mode: 0o755 });
    }
    writeFileSync(join(source, "install.sh"), readFileSync(join(repo, "install.sh")));
    mkdirSync(destination);
    writeFileSync(join(destination, "devo"), "old backend only");
    const result = Bun.spawnSync(["sh", join(source, "install.sh"), "--offline", "--no-modify-path", "--install-dir", destination], { stdout: "pipe", stderr: "pipe" });
    expect({ exit: result.exitCode, stderr: result.stderr.toString(), complete: files.every((file) => existsSync(join(destination, file))) }).toEqual({ exit: 0, stderr: "", complete: true });
    const second = Bun.spawnSync(["sh", join(source, "install.sh"), "--offline", "--no-modify-path", "--install-dir", destination], { stdout: "pipe", stderr: "pipe" });
    expect({ exit: second.exitCode, stderr: second.stderr.toString(), contents: readFileSync(join(destination, "tui/src/index.js"), "utf8") }).toEqual({ exit: 0, stderr: "", contents: "new" });
    const assets = join(root, "assets");
    mkdirSync(assets);
    writeFileSync(join(assets, "install.sh"), readFileSync(join(repo, "install.sh")));
    const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
    const platform = process.platform === "darwin" ? "apple-darwin" : "linux-musl";
    const archive = join(assets, `devo-tui-v0.2.0-${arch}-${platform}.tar.gz`);
    const packed = Bun.spawnSync(["tar", "-czf", archive, "-C", source, "."], { stdout: "pipe", stderr: "pipe" });
    expect({ exit: packed.exitCode, stderr: packed.stderr.toString() }).toEqual({ exit: 0, stderr: "" });
    const offlineDestination = join(root, "archive installed");
    const offline = Bun.spawnSync(["sh", join(assets, "install.sh"), "--offline", "--no-modify-path", "--install-dir", offlineDestination], { stdout: "pipe", stderr: "pipe" });
    expect({ exit: offline.exitCode, stderr: offline.stderr.toString(), complete: files.every((file) => existsSync(join(offlineDestination, file))) }).toEqual({ exit: 0, stderr: "", complete: true });
    if (process.platform === "linux") {
      renameSync(archive, archive.replace("-linux-musl", "-unknown-linux-musl"));
      const olderDestination = join(root, "older archive installed");
      const older = Bun.spawnSync(["sh", join(assets, "install.sh"), "--offline", "--no-modify-path", "--install-dir", olderDestination], { stdout: "pipe", stderr: "pipe" });
      expect({ exit: older.exitCode, stderr: older.stderr.toString(), complete: files.every(file => existsSync(join(olderDestination, file))) }).toEqual({ exit: 0, stderr: "", complete: true });
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
