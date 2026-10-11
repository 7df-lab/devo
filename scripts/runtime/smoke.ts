import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { WINDOWS_SANDBOX_HELPERS } from "./backend";

/** Exercise real private runtimes from a relocated installation without PATH tools. */
export function smokeBundle(bundle: string): void {
  const manifest = JSON.parse(readFileSync(join(bundle, "runtime/manifest.json"), "utf8"));
  const platform = manifest.target.includes("windows") ? "win32" : manifest.target.includes("darwin") ? "darwin" : "linux";
  const arch = manifest.target.startsWith("aarch64-") ? "arm64" : "x64";
  if (platform === "win32") {
    for (const name of WINDOWS_SANDBOX_HELPERS) {
      if (!existsSync(join(bundle, name))) throw new Error(`Incomplete Windows sandbox: ${name}`);
    }
  }
  const pythonRelative = platform === "win32" ? "runtime/python/python.exe" : "runtime/python/bin/python3";
  const nodeRelative = platform === "win32" ? "runtime/node/node.exe" : "runtime/node/bin/node";
  const runtimePath = (root: string, kind: string, relative: string) => {
    const reference = join(root, `runtime/${kind}.path`);
    return existsSync(reference) ? join(readFileSync(reference, "utf8").replace(/[\r\n]+$/, ""), platform === "win32" ? `${kind}.exe` : kind === "node" ? "bin/node" : "bin/python3") : join(root, relative);
  };
  for (const path of [runtimePath(bundle, "python", pythonRelative), runtimePath(bundle, "node", nodeRelative)]) {
    if (!existsSync(path)) throw new Error(`Incomplete runtime: ${path}`);
  }
  for (const name of ["tui/src/index.js", "runtime/python-site/rlm/repl.py", "runtime/python-site/sitecustomize.py", "runtime/python-site/dill/__init__.py"]) {
    if (!existsSync(join(bundle, name))) throw new Error(`Incomplete bundle: ${name}`);
  }
  if (platform !== process.platform || arch !== process.arch) {
    console.log(`Cross-target bundle structure verified: ${manifest.target}; execution requires a native ${platform}/${arch} machine`);
    return;
  }
  const work = mkdtempSync(join(tmpdir(), "devo-installed-smoke-"));
  try {
    const installed = join(work, "installation with spaces");
    cpSync(bundle, installed, { recursive: true });
    const cwd = join(work, "workspace");
    mkdirSync(cwd);
    const env = { ...process.env, PATH: cwd, PYTHONPATH: join(installed, "runtime/python-site"), PYTHONNOUSERSITE: "1" };
    delete env.PYTHONHOME;
    delete env.NODE_PATH;
    delete env.NODE_OPTIONS;
    const python = runtimePath(installed, "python", pythonRelative);
    const node = runtimePath(installed, "node", nodeRelative);
    const moduleProbe = `
      import { createRequire } from 'node:module';
      import { pathToFileURL } from 'node:url';
      import net from 'node:net';
      globalThis.fetch = () => { throw new Error('Network disabled in bundle QA'); };
      net.Socket.prototype.connect = function() { throw new Error('Network disabled in bundle QA'); };
      const root = process.argv[1];
      const require = createRequire(pathToFileURL(root + '/tui/package.json'));
      require('koffi');
      require('@mariozechner/clipboard');
      require('@silvia-odwyer/photon-node');
      await import(pathToFileURL(root + '/tui/src/host.js').href);
      console.log('Compiled TUI, WASM assets and native addons loaded offline');
    `;
    const js = Bun.spawnSync([node, "--input-type=module", "-e", moduleProbe, installed], { cwd, env, stdout: "pipe", stderr: "pipe", timeout: 60_000 });
    if (js.exitCode !== 0) throw new Error(`Private Node smoke failed: ${js.stderr.toString()}`);
    console.log(js.stdout.toString().trim());
    const snapshot = join(work, "namespace.dill");
    const requests = [
      { type: "execute", id: "execute", code: "import socket\ndef deny(*args, **kwargs):\n    raise RuntimeError('Network disabled in bundle QA')\nsocket.socket.connect = deny\nimport asyncio, mcp, tyro, dill, rlm.mcp, refine, compact, goal, agent_observe\nawait asyncio.sleep(0.01)\nbundle_answer = 40 + 2\nprint('PRIVATE_KERNEL_OK')" },
      { type: "snapshot", id: "snapshot", path: snapshot, manifest_path: snapshot + ".json" },
      { type: "execute", id: "change", code: "bundle_answer = 0" },
      { type: "restore", id: "restore", path: snapshot },
      { type: "execute", id: "verify", code: "assert bundle_answer == 42\nprint('PRIVATE_RESTORE_OK')" },
      { type: "shutdown", id: "shutdown" },
    ];
    const kernel = Bun.spawnSync([python, "-u", "-m", "rlm.repl"], {
      cwd, env, stdin: Buffer.from(requests.map((request) => JSON.stringify(request)).join("\n") + "\n"),
      stdout: "pipe", stderr: "pipe", timeout: 60_000,
    });
    const events = kernel.stdout.toString().trim().split(/\r?\n/).map((line) => JSON.parse(line));
    if (kernel.exitCode !== 0 || events.some((event) => event.event === "error")) {
      throw new Error(`Private Python kernel smoke failed: ${kernel.stderr}\n${kernel.stdout}`);
    }
    if (!events.some((event) => event.event === "ready" && event.protocol === 3) ||
        !requests.slice(0, -1).every((request) => events.some((event) => event.event === "done" && event.id === request.id && event.status === "ok")) ||
        !events.some((event) => event.event === "stdout" && event.text.includes("PRIVATE_RESTORE_OK"))) {
      throw new Error(`Kernel handshake/execution/snapshot/restore incomplete: ${kernel.stdout}`);
    }
    console.log(`Private Python ${manifest.python}: protocol 3, execution, snapshot and restore passed offline`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (import.meta.main) smokeBundle(resolve(process.argv[2] ?? ""));
