import assert from "node:assert/strict";
import test from "node:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { createOwnedSmokeTmux } from "./owned-smoke-tmux.mjs";

function fixture(t, spawn) {
  const directory = mkdtempSync(join(tmpdir(), "owned-tmux-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return { directory, server: createOwnedSmokeTmux({ directory, executable: "test-tmux", spawn }) };
}

test("one isolated server stays nonempty across product session restarts", t => {
  const sessions = new Set();
  const calls = [];
  let started = false;
  const f = fixture(t, (executable, args, options) => {
    calls.push(args);
    assert.equal(executable, "test-tmux");
    assert.deepEqual(args.slice(0, 4), ["-L", basename(f.directory), "-f", join(f.directory, "tmux.conf")]);
    assert.deepEqual(options, { encoding: "utf8", timeout: 10000 });
    const command = args[4];
    if (command === "new-session") {
      // Model exit-empty: after the last session exits, a reconnect must fail.
      if (started && sessions.size === 0) return { status: 1, stderr: "server exited unexpectedly" };
      started = true;
      sessions.add(args[args.indexOf("-s") + 1]);
    } else if (command === "kill-session") {
      sessions.delete(args[args.indexOf("-t") + 1]);
    } else if (command === "kill-server") {
      sessions.clear();
    } else assert.fail(`unexpected command ${command}`);
    return { status: 0, stdout: "", stderr: "" };
  });
  assert.equal(readFileSync(join(f.directory, "tmux.conf"), "utf8"), "");
  f.server.start();
  assert.deepEqual([...sessions], ["smoke-keeper"]);
  assert.deepEqual(calls[0].slice(-3), [process.execPath, "-e", "setInterval(() => {}, 60000)"]);
  for (let i = 0; i < 100; i++) {
    f.server.mux(["new-session", "-d", "-s", `product-${i}`, "--", "devo"]);
    f.server.mux(["kill-session", "-t", `product-${i}`]);
    assert.deepEqual([...sessions], ["smoke-keeper"]);
  }
  f.server.close(); f.server.close();
  assert.equal(sessions.size, 0);
  assert.equal(calls.filter(args => args[4] === "kill-server").length, 1);
});

test("partial startup errors are not retried and cleanup targets only the owned socket", t => {
  const calls = [];
  const f = fixture(t, (_executable, args) => {
    calls.push(args);
    return { status: 1, stderr: "server exited unexpectedly" };
  });
  assert.throws(() => f.server.start(), /new-session: server exited unexpectedly/);
  f.server.close();
  assert.deepEqual(calls.map(args => args[4]), ["new-session", "kill-server"]);
  assert.ok(calls.every(args => args[0] === "-L" && args[1] === basename(f.directory)));
});

test("startup cannot be repeated while owned, and unused cleanup starts no server", t => {
  const calls = [];
  const f = fixture(t, (_executable, args) => { calls.push(args); return { status: 0 }; });
  f.server.close(); assert.equal(calls.length, 0);
  f.server.start();
  assert.throws(() => f.server.start(), /already started/);
  f.server.close();
  assert.equal(calls.length, 2);
});

test("distinct runs use distinct sockets and command failures remain failures", t => {
  const calls = [];
  const spawn = (_executable, args) => { calls.push(args); return { error: new Error("test failure") }; };
  const a = fixture(t, spawn), b = fixture(t, spawn);
  assert.throws(() => a.server.mux(["capture-pane"]), /test failure/);
  assert.throws(() => b.server.mux(["capture-pane"]), /test failure/);
  assert.notEqual(calls[0][1], calls[1][1]);
});
