import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, join, relative } from "node:path";
import { pruneNodeInputs } from "./prune";

export function run(program: string, args: string[], cwd?: string): void {
  const result = Bun.spawnSync([program, ...args], { cwd, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) throw new Error(`${program} failed (${result.exitCode})`);
}

/** Preserve assets and module boundaries: dynamic imports and native addons need real files. */
export function compileTree(directory: string): void {
  const transpiler = new Bun.Transpiler({ loader: "ts", target: "node" });
  for (const item of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, item.name);
    if (item.isDirectory()) {
      if (item.name !== "node_modules") compileTree(path);
    } else if (item.name.endsWith(".ts")) {
      if (!/\.(test|spec|d)\.ts$/.test(item.name)) {
        writeFileSync(path.slice(0, -3) + ".js", transpiler.transformSync(readFileSync(path, "utf8")));
      }
      rmSync(path);
    } else if (item.name === "package.json") {
      const pkg = JSON.parse(readFileSync(path, "utf8"));
      for (const key of ["main", "types", "exports"]) {
        if (pkg[key]) pkg[key] = JSON.parse(JSON.stringify(pkg[key]).replace(/\.ts(?=")/g, ".js"));
      }
      delete pkg.devDependencies;
      writeFileSync(path, JSON.stringify(pkg, null, 2) + "\n");
    }
  }
}

export function buildTui(repo: string, output: string, work: string, platform: string, arch: string): void {
  const source = join(repo, "apps/tui");
  const staging = join(work, "tui-source");
  mkdirSync(staging, { recursive: true });
  for (const name of ["package.json", "package-lock.json", "src", "lib"]) {
    cpSync(join(source, name), join(staging, name), {
      recursive: true,
      filter: (path) => !["node_modules", "dist", ".git"].includes(basename(path)) && !/\.(test|spec)\.ts$/.test(path),
    });
  }
  // npm validates lockfile integrity and selects target optional native packages.
  run(process.platform === "win32" ? "npm.cmd" : "npm", [
    "ci", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund", `--os=${platform}`, `--cpu=${arch}`,
  ], staging);
  // Materialize file: package links so the archive has no links to the checkout.
  cpSync(staging, output, { recursive: true, dereference: true });
  compileTree(output);
  const scope = join(output, "node_modules/@earendil-works");
  for (const name of readdirSync(scope)) compileTree(join(scope, name));
  // Runtime imports use the materialized packages. The staging originals duplicate them.
  rmSync(join(output, "lib"), { recursive: true });
  pruneNodeInputs(join(output, "node_modules"));
  rmSync(join(output, "package-lock.json"));

  const koffi = join(output, "node_modules/koffi/build/koffi");
  const target = `${platform}_${arch}`;
  if (!existsSync(join(koffi, target, "koffi.node"))) throw new Error(`Missing Koffi native addon: ${target}`);
  for (const name of readdirSync(koffi)) {
    if (name !== target) rmSync(join(koffi, name), { recursive: true, force: true });
  }
  const clipboard = join(output, "node_modules/@mariozechner", `clipboard-${platform}-${arch}${platform === "win32" ? "-msvc" : platform === "linux" ? "-gnu" : ""}`);
  if (!existsSync(clipboard)) throw new Error(`Missing clipboard native addon: ${relative(output, clipboard)}`);
}
