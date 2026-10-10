import { createHash } from "node:crypto";
import { createReadStream, createWriteStream, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { createGzip } from "node:zlib";

function tarHeader(name: string, size: number, mode: number, type: string, link = ""): Buffer {
  const header = Buffer.alloc(512);
  header.write(name, 0, 100);
  for (const [offset, length, value] of [[100, 8, mode], [108, 8, 0], [116, 8, 0], [124, 12, size], [136, 12, 0]]) {
    header.write(value.toString(8).padStart(length - 1, "0") + "\0", offset, length);
  }
  header.fill(32, 148, 156);
  header.write(type, 156, 1);
  header.write(link, 157, 100);
  header.write("ustar\0", 257, 6);
  header.write("00", 263, 2);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  header.write(checksum.toString(8).padStart(6, "0") + "\0 ", 148, 8);
  return header;
}

/** Stable gzip/tar metadata lets unchanged runtimes share a digest across releases. */
export async function archiveTree(root: string, paths: string[], output: string): Promise<void> {
  let serial = 0;
  async function* entries(relative: string): AsyncGenerator<Buffer> {
    const path = join(root, relative);
    const info = lstatSync(path);
    const name = relative.replaceAll("\\", "/") + (info.isDirectory() ? "/" : "");
    const link = info.isSymbolicLink() ? readlinkSync(path) : "";
    if (!info.isFile() && !info.isDirectory() && !info.isSymbolicLink()) throw new Error(`Unsupported runtime entry: ${path}`);
    const pax: Buffer[] = [];
    for (const [key, value] of [["path", name], ["linkpath", link]]) {
      if (Buffer.byteLength(value) <= 100) continue;
      const record = ` ${key}=${value}\n`;
      let length = Buffer.byteLength(record) + 1;
      while (length !== Buffer.byteLength(record) + String(length).length) length = Buffer.byteLength(record) + String(length).length;
      pax.push(Buffer.from(`${length}${record}`));
    }
    if (pax.length) {
      const data = Buffer.concat(pax);
      yield tarHeader(`PaxHeaders/${serial}`, data.length, 0o644, "x");
      yield data;
      yield Buffer.alloc((512 - data.length % 512) % 512);
    }
    const size = info.isFile() ? info.size : 0;
    yield tarHeader(Buffer.byteLength(name) > 100 ? `entry${serial}` : name, size,
      info.isDirectory() || (info.mode & 0o111) ? 0o755 : 0o644,
      info.isDirectory() ? "5" : info.isSymbolicLink() ? "2" : "0", Buffer.byteLength(link) > 100 ? "" : link);
    serial++;
    if (info.isFile()) {
      for await (const chunk of createReadStream(path)) yield chunk as Buffer;
      yield Buffer.alloc((512 - size % 512) % 512);
    } else if (info.isDirectory()) {
      for (const child of readdirSync(path).sort()) yield* entries(`${relative}/${child}`);
    }
  }
  async function* archive() {
    for (const path of paths.sort()) yield* entries(path);
    yield Buffer.alloc(1024);
  }
  await pipeline(Readable.from(archive()), createGzip({ level: 9 }), createWriteStream(output));
}

/** Emit the small online app and independently cacheable interpreter packs. */
export async function buildPacks(bundle: string, output: string): Promise<string> {
  const manifest = JSON.parse(readFileSync(join(bundle, "runtime/manifest.json"), "utf8"));
  mkdirSync(output, { recursive: true });
  const lines = ["devo-install-v1"];
  for (const kind of ["app", "node", "python"] as const) {
    const partial = join(output, `${kind}.partial`);
    // App-specific Python dependencies/skills stay with the app, independently
    // of the interpreter and standard library shared across releases.
    const paths = kind === "app" ? readdirSync(bundle).filter(name => name !== "runtime")
      .concat(readdirSync(join(bundle, "runtime")).filter(name => !["node", "python", "node.path", "python.path"].includes(name)).map(name => `runtime/${name}`))
      : [`runtime/${kind}`];
    await archiveTree(bundle, paths, partial);
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(partial)) hash.update(chunk);
    const digest = hash.digest("hex");
    const name = kind === "app" ? `devo-tui-app-v${manifest.version}-${manifest.target}.tar.gz`
      : `devo-runtime-${kind}-${manifest.target}-${digest}.tar.gz`;
    const destination = join(output, name);
    // Overwriting the same runtime digest is harmless; app archives are versioned.
    rmSync(destination, { force: true });
    renameSync(partial, destination);
    lines.push(`${kind} ${digest} ${name}`);
  }
  const index = join(output, `devo-tui-v${manifest.version}-${manifest.target}.install.txt`);
  writeFileSync(index, lines.join("\n") + "\n");
  console.log(`Online installation index: ${basename(index)}`);
  return index;
}

if (import.meta.main) {
  if (!process.argv[2] || !process.argv[3]) throw new Error("Usage: bun scripts/runtime/packs.ts <full-bundle> <asset-directory>");
  await buildPacks(resolve(process.argv[2]), resolve(process.argv[3]));
}
