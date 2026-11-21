import assert from "node:assert/strict";
import test from "node:test";
import { PassThrough } from "node:stream";
import { setTimeout as sleep } from "node:timers/promises";
import type { Terminal } from "@earendil-works/pi-tui";
import { runWorkspaceTrustPrompt } from "./workspace-trust-prompt.js";

function fixture(initialColumns = 100, initialRows = 30) {
  const writes: string[] = [];
  let onInput = (_data: string) => {};
  let onResize = () => {};
  let stops = 0;
  let columns = initialColumns;
  let rows = initialRows;
  const input = new PassThrough();
  const terminal: Terminal = {
    get columns() { return columns; }, get rows() { return rows; }, kittyProtocolActive: false, altScreenActive: false, mouseTrackingActive: false,
    start(data, resize) { onInput = data; onResize = resize; },
    stop() { stops++; },
    async drainInput() {},
    write(data) { writes.push(data); },
    moveBy() {}, hideCursor() {}, showCursor() {}, clearLine() {}, clearFromCursor() {}, clearScreen() {},
    enterAltScreen() {}, leaveAltScreen() {}, setMouseTracking() {}, setTitle() {}, setProgress() {},
  };
  return {
    terminal, input, writes, key: (data: string) => onInput(data),
    resize: (width: number, height = rows) => { columns = width; rows = height; onResize(); },
    stops: () => stops,
  };
}

for (const [keys, expected] of [
  [["\r"], false],
  [["y", "e", "s", "\r"], false],
  [["\x1b[B", "\r"], true],
  [["\x1b[A", "\r"], true],
  [["\x1b[B", "\x1b[A", "\r"], false],
  [["\x1b[B", "\x1b"], false],
  [["\x1b[B", "\x03"], false],
  [["\x1b[B", "\x04"], false],
  [["\x1b[200~\x1b[B\r\x1b[201~", "\r"], false],
  [["\x1b[1;1:3B", "\r"], false],
] as const) {
  test(`selector keys ${JSON.stringify(keys)} => ${expected}`, async () => {
    const f = fixture();
    const result = runWorkspaceTrustPrompt("/project", f.terminal, f.input);
    for (const key of keys) f.key(key);
    assert.equal(await result, expected);
    assert.equal(f.stops(), 1);
    assert.deepEqual(["end", "close", "error"].map(event => f.input.listenerCount(event)), [0, 0, 0]);
    f.key("\x1b[B"); f.key("\r");
    assert.equal(f.stops(), 1);
  });
}
for (const event of ["end", "close", "error"] as const) {
  test(`${event} declines after selecting Yes and releases the terminal`, async () => {
    const f = fixture();
    const result = runWorkspaceTrustPrompt("/project", f.terminal, f.input);
    f.key("\x1b[B"); f.input.emit(event, new Error("closed"));
    assert.equal(await result, false);
    assert.equal(f.stops(), 1);
  });
}
test("already closed input cannot begin admission", async () => {
  const f = fixture(); f.input.destroy();
  assert.equal(await runWorkspaceTrustPrompt("/project", f.terminal, f.input), false);
  assert.equal(f.stops(), 0);
});
test("startup failure declines and restores the terminal", async () => {
  const f = fixture(); f.terminal.start = () => { throw new Error("raw mode unavailable"); };
  assert.equal(await runWorkspaceTrustPrompt("/project", f.terminal, f.input), false);
  assert.equal(f.stops(), 1);
});
test("startup copy, safe path, default choice and resize repaint", async () => {
  const f = fixture();
  const result = runWorkspaceTrustPrompt("/project\x1b[2J\x9b31m", f.terminal, f.input);
  await sleep(40);
  const initial = f.writes.join("");
  assert.match(initial, /› No, exit/);
  assert.match(initial, /Yes, I trust this folder/);
  assert.match(initial, /\\u001b\[2J\\u009b31m/);
  assert.match(initial, /read and edit files/);
  assert.doesNotMatch(initial, /Type yes/);
  f.resize(40); f.key("\x1b[B");
  await sleep(40);
  assert.match(f.writes.join(""), /› Yes, I trust this folder/);
  f.key("\x1b"); assert.equal(await result, false);
});

test("a short pane keeps the folder, warning and default decision visible", async () => {
  const f = fixture(52, 9);
  const path = `/benchmark/${"long-folder/".repeat(6)}target-workspace`;
  const result = runWorkspaceTrustPrompt(path, f.terminal, f.input);
  await sleep(40);
  const initial = f.writes.join("");
  assert.match(initial, /Workspace trust/);
  assert.match(initial, /\/benchmark\//);
  assert.match(initial, /target-workspace/);
  assert.match(initial, /Trust does not enable a sandbox/);
  assert.match(initial, /› No, exit/);
  assert.match(initial, /Yes, I trust this folder/);
  assert.match(initial, /PgUp\/PgDn details/);

  f.writes.length = 0;
  f.key("\x1b[6~");
  await sleep(40);
  assert.match(f.writes.join(""), /Only continue if you trust/);
  f.writes.length = 0;
  f.key("\x1b[5~");
  await sleep(40);
  assert.match(f.writes.join(""), /\/benchmark\//);
  f.key("\r");
  assert.equal(await result, false);
});

test("an unusably small pane cannot approve trust using invisible choices", async () => {
  const f = fixture(30, 4);
  const result = runWorkspaceTrustPrompt("/project", f.terminal, f.input);
  await sleep(40);
  assert.match(f.writes.join(""), /Resize to review workspace/);
  f.key("\x1b[B"); f.key("\r");
  assert.equal(f.stops(), 0);
  f.resize(52, 9);
  await sleep(40);
  assert.match(f.writes.join(""), /› No, exit/);
  f.key("\x1b[B"); f.key("\r");
  assert.equal(await result, true);
});
