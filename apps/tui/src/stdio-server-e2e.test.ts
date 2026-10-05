/**
 * Real-server stdio smoke: spawn `devo server --transport stdio` and exercise
 * initialize → session/new → subscription/create (kind) → schedule upsert → export.
 * No TTY required.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "../../..");
const serverBin = process.env.DEVO_SERVER_BIN
  || (existsSync(join(repoRoot, "target/debug/devo.exe"))
    ? join(repoRoot, "target/debug/devo.exe")
    : join(repoRoot, "target/debug/devo"));

test("real stdio server: boot, subscribe kind, schedule, export", async (t) => {
  if (!existsSync(serverBin)) {
    t.skip(`server binary missing at ${serverBin}`);
    return;
  }

  // Isolated home: the heartbeat upsert below writes $DEVO_HOME/schedules.json,
  // which is process-shared, so the test must not touch the user's store.
  const home = mkdtempSync(join(tmpdir(), "devo-stdio-e2e-"));

  const child = spawn(serverBin, ["server", "--transport", "stdio"], {
    cwd: repoRoot,
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, DEVO_HOME: home },
  });
  assert.ok(child.stdin && child.stdout);

  const stderr: string[] = [];
  child.stderr?.setEncoding("utf8");
  child.stderr?.on("data", (c: string) => stderr.push(c));

  const conn = NativeAgentConnection.create({
    writeLine: (line) => {
      child.stdin!.write(`${line}\n`);
    },
    cwd: repoRoot,
  });
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => conn.pushChunk(chunk));

  const onExit = new Promise<number | null>((resolveExit) => {
    child.on("exit", (code) => resolveExit(code));
  });

  // Cleared on success so the guard timer does not keep the process alive
  // after the test finishes (node:test waits for a quiet event loop).
  let timeoutHandle: ReturnType<typeof setTimeout> | undefined;

  try {
    await Promise.race([
      (async () => {
        await conn.initialize({ name: "stdio-e2e", version: "0" });
        const created = await conn.newSession();
        assert.equal(created.cancelled, false);
        const state = await conn.getState();
        assert.ok(state.sessionId);

        await conn.setSessionName("stdio-e2e");
        const hb = await conn.setHeartbeat("every 1h", "e2e heartbeat", "steer");
        assert.ok(hb?.id);

        const path = await conn.exportToJsonl();
        assert.ok(path);
        assert.match(String(path), /\.jsonl$/i);

        await conn.abort();
        await conn.dispose();
      })(),
      onExit.then((code) => {
        throw new Error(`server exited early code=${code} stderr=${stderr.join("")}`);
      }),
      new Promise((_, reject) => {
        timeoutHandle = setTimeout(
          () => reject(new Error(`timeout stderr=${stderr.join("")}`)),
          45_000,
        );
      }),
    ]);
  } finally {
    if (timeoutHandle) clearTimeout(timeoutHandle);
    if (!child.killed) child.kill();
    rmSync(home, { recursive: true, force: true });
  }
});
