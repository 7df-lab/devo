import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
      $rejected = $false
      try { Install-DevoBundle -Source ${quote(root)} -InstallDir ${quote(destination)} } catch { $rejected = $true }
      Set-Content -LiteralPath ${quote(join(destination, "runtime/manifest.json"))} -Value 'old runtime'
      $lock = [IO.File]::Open(${quote(join(destination, "rg.exe"))}, 'Open', 'ReadWrite', 'None')
      $failed = $false
      try { Install-DevoBundle -Source ${quote(source)} -InstallDir ${quote(destination)} } catch { $failed = $true } finally { $lock.Dispose() }
      $restored = (Get-Content -LiteralPath ${quote(join(destination, "runtime/manifest.json"))} -Raw).Trim() -eq 'old runtime'
      @{complete=$complete; rejected=$rejected; rolledBack=($failed -and $restored)} | ConvertTo-Json -Compress
    `;
    const path = join(root, "test.ps1");
    writeFileSync(path, script);
    const result = Bun.spawnSync([join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/powershell.exe"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path], { stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(result.stderr.toString());
    expect({ exit: result.exitCode, stderr: result.stderr.toString(), checks: JSON.parse(result.stdout.toString()) }).toEqual({ exit: 0, stderr: "", checks: { complete: true, rejected: true, rolledBack: true } });
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
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
