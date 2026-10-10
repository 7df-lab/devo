import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { buildPacks } from "./packs";
import { installOnline, writeChecksums } from "./installer-fixture";
import { smokeBundle } from "./smoke";

const bundle = resolve(process.argv[2] ?? "");
const manifest = JSON.parse(readFileSync(join(bundle, "runtime/manifest.json"), "utf8"));
const work = mkdtempSync(join(tmpdir(), "devo shared runtime QA "));
const requests: string[] = [];
const assets = join(work, "assets");
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
  const name = basename(new URL(request.url).pathname);
  requests.push(name);
  const file = Bun.file(join(assets, name));
  return await file.exists() ? new Response(file) : new Response("Not found", { status: 404 });
} });
try {
  await buildPacks(bundle, assets);
  writeChecksums(assets);
  // Cross-target archive creation is still checked; real execution requires a
  // matching native platform, like the complete-bundle smoke test.
  const platform = manifest.target.includes("windows") ? "win32" : manifest.target.includes("darwin") ? "darwin" : "linux";
  const arch = manifest.target.startsWith("aarch64-") ? "arm64" : "x64";
  if (platform !== process.platform || arch !== process.arch) {
    console.log(`Cross-target runtime packs verified: ${manifest.target}`);
  } else {
    const destination = join(work, "installation with spaces");
    const cache = join(work, "shared runtime cache");
    const origin = `http://127.0.0.1:${server.port}`;
    const install = await installOnline(work, destination, cache, origin, manifest.target, `v${manifest.version}`);
    if (install.exit !== 0) throw new Error(`${install.stdout}\n${install.stderr}`);
    console.log(install.stdout.trim());
    smokeBundle(destination);
    requests.length = 0;
    const reinstall = await installOnline(work, destination, cache, origin, manifest.target, `v${manifest.version}`);
    if (reinstall.exit !== 0 || requests.some(name => name.startsWith("devo-runtime-"))) throw new Error(`Runtime cache reuse failed: ${reinstall.stdout}\n${reinstall.stderr}\n${requests}`);
    console.log("Reinstallation reused both verified runtimes without downloading or extracting them");
  }
} finally {
  server.stop(true);
  rmSync(work, { recursive: true, force: true });
}
