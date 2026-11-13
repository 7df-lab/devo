/** Test-only clipboard/conversion I/O substitute; product event handlers are untouched.
 * Loaded only by the clipboard tmux scenario via an isolated node wrapper. This is not an
 * operating-system clipboard test. File handshakes make each race deterministic.
 */
import { mock } from "node:test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const root = process.env.DEVO_CLIPBOARD_SMOKE_DIR;
if (!root) throw new Error("clipboard smoke requires an isolated control directory");
const source = new URL("../lib/coding-agent/src/utils/", import.meta.url);
const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9l8AAAAASUVORK5CYII=", "base64");
let request;
async function gate(stage) {
  if (request.stage !== stage) return;
  const key = `${request.id}-${stage}`;
  writeFileSync(join(root, `${key}.started`), "started");
  const deadline = Date.now() + 60_000;
  while (!existsSync(join(root, `${key}.release`))) {
    if (Date.now() > deadline) throw new Error(`clipboard smoke gate timed out: ${key}`);
    await sleep(20);
  }
  writeFileSync(join(root, `${key}.finished`), "finished");
}
mock.module(new URL("clipboard-image.ts", source).href, {
  namedExports: {
    isWaylandSession: () => false,
    readClipboardImage: async () => {
      request = JSON.parse(readFileSync(join(root, "request.json"), "utf8"));
      await gate("read");
      return { bytes: png, mimeType: "image/png" };
    },
  },
});
mock.module(new URL("image-resize.ts", source).href, {
  namedExports: { resizeImage: async image => {
    await gate("resize");
    return { data: image.data, mimeType: image.mimeType };
  } },
});
