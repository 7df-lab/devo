import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

export const repo = resolve(import.meta.dir, "../..");

export function writeChecksums(assets: string): void {
  writeFileSync(join(assets, "SHA256SUMS.txt"), readdirSync(assets).filter(name => name !== "SHA256SUMS.txt").sort()
    .map(name => `${createHash("sha256").update(readFileSync(join(assets, name))).digest("hex")}  ${name}`).join("\n") + "\n");
}

/** Run the actual network/install functions without modifying the user's PATH. */
export async function installOnline(root: string, destination: string, cache: string, origin: string, target: string, version = "v0.2.0", mode: "functions" | "entrypoint" = "functions") {
  const work = join(root, `work-${crypto.randomUUID()}`);
  mkdirSync(work);
  const windows = process.platform === "win32";
  const quote = (value: string) => windows ? `'${value.replaceAll("'", "''")}'` : `'${value.replaceAll("'", "'\\''")}'`;
  const original = readFileSync(join(repo, windows ? "install.ps1" : "install.sh"), "utf8");
  const source = mode === "entrypoint" ? original : original.replace(windows ? /\nMain\s*$/ : /\nmain\s*$/, "\n");
  const script = windows
    ? source + `
      ${mode === "functions" ? `Install-DevoOnline -Target ${quote(target)} -ResolvedVersion ${quote(version)} -InstallDir ${quote(destination)} -TempRoot ${quote(work)}` : ""}
      if (-not (Test-DevoBundle -Directory ${quote(destination)})) { throw 'Installed bundle is incomplete' }
    `
    : source + `
      ${mode === "functions" ? `download_and_install ${quote(target)} ${quote(version)}` : ""}
      bundle_complete ${quote(destination)} || die 'Installed bundle is incomplete'
    `;
  const path = join(work, windows ? "test.ps1" : "test.sh");
  writeFileSync(path, script);
  const child = Bun.spawn(windows
    ? [join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/powershell.exe"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path]
    : ["sh", path, "--no-modify-path"], {
    stdout: "pipe", stderr: "pipe",
    env: { ...process.env, VERSION: version, DEVO_RUNTIME_CACHE: cache, DEVO_RELEASE_BASE_URL: origin, DEVO_RUNTIME_BASE_URL: `${origin}/runtime-packs`, DEVO_INSTALL_DIR: destination, DEVO_NO_MODIFY_PATH: "1",
      ...(windows ? { PSModulePath: join(process.env.SystemRoot!, "System32/WindowsPowerShell/v1.0/Modules") } : {}) },
  });
  const [exit, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  return { exit, stdout, stderr };
}
