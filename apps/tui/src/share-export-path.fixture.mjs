import { mock } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import * as fs from "node:fs";
import os from "node:os";
import path from "node:path";
const base = new URL("../lib/coding-agent/src/", import.meta.url);
const original = await import(new URL("utils/child-process.ts", base));
let scenario, mode, uploaded, requested;
mock.module(new URL("utils/child-process.ts", base).href, { namedExports: {
  ...original,
  spawnSyncHidden: () => ({ status: 0 }),
  spawnHidden: (_command, args) => {
    uploaded = args.at(-1);
    assert.equal(fs.readFileSync(uploaded, "utf8"), "current session");
    const child = new EventEmitter();
    child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
    child.kill = () => { setImmediate(() => child.emit("close", null)); };
    setImmediate(() => {
      if (scenario === "cancel") { mode.loader.handleInput("\x1b"); return; }
      if (scenario === "spawn-error") { child.emit("error", new Error("spawn failed")); return; }
      child.stdout.emit("data", "https://gist.github.com/test/abc\n");
      child.stderr.emit("data", scenario === "upload-failure" ? "upload failed" : "");
      child.emit("close", scenario === "upload-failure" ? 1 : 0);
    });
    return child;
  },
} });
const { InteractiveMode } = await import(new URL("modes/interactive/interactive-mode.ts", base));
const { initTheme } = await import(new URL("modes/interactive/theme/theme.ts", base));
initTheme("dark");
const root = fs.mkdtempSync(path.join(os.tmpdir(), "share-fixture-"));
try {
  // The old fixed name is deliberately stale; it must never be uploaded or deleted.
  const stale = path.join(os.tmpdir(), "session.html");
  fs.writeFileSync(stale, "stale unrelated session");
  for (scenario of ["success", "upload-failure", "spawn-error", "cancel", "export-failure", "copy-failure", "same-path"]) {
    uploaded = undefined; requested = undefined;
    const exported = path.join(root, `${scenario}.html`);
    const statuses = [], errors = [];
    mode = Object.create(InteractiveMode.prototype);
    Object.assign(mode, {
      editor: {},
      editorContainer: { clear() {}, addChild(value) { if (value !== mode.editor) mode.loader = value; } },
      ui: { setFocus() {}, requestRender() {} },
      showStatus: value => statuses.push(value), showError: value => errors.push(value),
      agentConnection: { exportToHtml: async output => {
        requested = output;
        if (scenario === "export-failure") throw new Error("export failed");
        const destination = scenario === "same-path" ? output : exported;
        if (scenario !== "copy-failure") fs.writeFileSync(destination, "current session");
        return destination;
      } },
    });
    await mode.handleShareCommand();
    assert.equal(fs.readFileSync(stale, "utf8"), "stale unrelated session");
    assert.notEqual(requested, stale, "share must request its own unique temporary name");
    assert.equal(uploaded, ["export-failure", "copy-failure"].includes(scenario) ? undefined : requested);
    assert.equal(path.basename(requested), "session.html");
    assert.equal(fs.existsSync(path.dirname(requested)), false);
    assert.equal(fs.existsSync(exported), false, "own export must be cleaned up");
    if (["success", "same-path"].includes(scenario)) assert.ok(statuses.some(s => s.includes("Share URL:")));
    else if (scenario === "cancel") assert.ok(statuses.includes("Share cancelled"));
    else assert.equal(errors.length, 1);
    console.log(`${scenario}: verified`);
  }
} finally { fs.rmSync(root, { recursive: true, force: true }); }
