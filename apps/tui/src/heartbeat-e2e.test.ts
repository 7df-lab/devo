/**
 * Real-server heartbeat firing e2e: spawn `devo server --transport stdio`
 * with an isolated DEVO_HOME, set an unlabeled heartbeat on a 1s interval,
 * wait for the 15s schedule wake loop to claim and deliver it, then verify
 * the injected user message in the exported rollout and clear the job.
 * No TTY required.
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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

/** The server schedule wake loop ticks every 15s; a 1s heartbeat first fires on the tick after it comes due. */
const FIRE_TIMEOUT_MS = 45_000;
const POLL_INTERVAL_MS = 2_000;
/** Delivery runs queue push -> turn start -> user bubble; give it a moment before exporting. */
const SETTLE_AFTER_FIRE_MS = 3_000;
const OVERALL_TIMEOUT_MS = 90_000;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

test("real stdio server: heartbeat fires, delivers, and clears", { timeout: OVERALL_TIMEOUT_MS }, async (t) => {
  if (!existsSync(serverBin)) {
    t.skip(`server binary missing at ${serverBin}`);
    return;
  }

  // Isolated home: the schedule store is $DEVO_HOME/schedules.json and is
  // process-shared, so a firing 1s heartbeat must never touch the user's.
  const home = mkdtempSync(join(tmpdir(), "devo-heartbeat-e2e-"));

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
        await conn.initialize({ name: "heartbeat-e2e", version: "0" });
        const created = await conn.newSession();
        assert.equal(created.cancelled, false);
        assert.ok((await conn.getState()).sessionId);

        const instruction = `e2e heartbeat fire probe ${Date.now()}`;
        const hb = await conn.setHeartbeat("every 1s", instruction, "follow_up");
        assert.ok(hb.id);
        assert.equal(hb.runCount, 0);

        // Wait for the wake loop to claim the job: runCount advances on claim.
        let firedJob: Awaited<ReturnType<typeof conn.getHeartbeat>> | undefined;
        const deadline = Date.now() + FIRE_TIMEOUT_MS;
        while (Date.now() < deadline) {
          firedJob = await conn.getHeartbeat();
          if ((firedJob?.runCount ?? 0) >= 1) break;
          await sleep(POLL_INTERVAL_MS);
        }
        assert.ok(
          (firedJob?.runCount ?? 0) >= 1 && firedJob?.lastRunAt,
          `heartbeat must fire via the wake loop (job=${JSON.stringify(firedJob)}) stderr=${stderr.join("")}`,
        );

        // The fire must deliver: the instruction lands as a user message in
        // the rollout, independent of whether a model call then succeeds.
        await sleep(SETTLE_AFTER_FIRE_MS);
        const exportPath = await conn.exportToJsonl();
        assert.match(String(exportPath), /\.jsonl$/i);
        const rollout = readFileSync(exportPath, "utf8");
        assert.ok(
          rollout.includes(instruction),
          `exported rollout must contain the delivered heartbeat instruction stderr=${stderr.join("")}`,
        );

        // Clear removes the unlabeled heartbeat for this session.
        await conn.updateHeartbeat("clear");
        assert.equal(
          await conn.getHeartbeat(),
          undefined,
          `cleared heartbeat must no longer be listed stderr=${stderr.join("")}`,
        );

        await conn.abort();
        await conn.dispose();
      })(),
      onExit.then((code) => {
        throw new Error(`server exited early code=${code} stderr=${stderr.join("")}`);
      }),
      new Promise((_, reject) => {
        timeoutHandle = setTimeout(
          () => reject(new Error(`timeout stderr=${stderr.join("")}`)),
          OVERALL_TIMEOUT_MS,
        );
      }),
    ]);
  } finally {
    if (timeoutHandle) clearTimeout(timeoutHandle);
    if (child.exitCode === null && !child.killed) child.kill();
    await onExit;
    rmSync(home, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
