/**
 * Real-server stdio: session/import appends rewritten rollout lines and
 * resume loads them into session/items/list (Prime import/open parity).
 */

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { NativeAgentConnection } from "./native-agent-connection.js";
import { resolveDevoHome } from "./host.js";

const repoRoot = resolve("../..");
const serverBin = join(repoRoot, "target/debug/devo.exe");

test("stdio import appends non-meta rollout lines", async (t) => {
  if (!fs.existsSync(serverBin)) {
    t.skip(`server binary missing at ${serverBin}`);
    return;
  }

  const sessionsDir = join(resolveDevoHome(), "sessions");
  const donors = fs.existsSync(sessionsDir)
    ? fs
        .readdirSync(sessionsDir)
        .filter((name) => name.endsWith(".jsonl"))
        .map((name) => join(sessionsDir, name))
        .map((path) => ({ path, size: fs.statSync(path).size }))
        .filter((entry) => entry.size > 5_000)
        .sort((a, b) => b.size - a.size)
    : [];
  if (donors.length === 0) {
    t.skip("no donor session rollouts under ~/.devo/sessions");
    return;
  }

  const fixtureDir = fs.mkdtempSync(join(repoRoot, "import-fixture-"));
  const fixture = join(fixtureDir, "import-fixture.jsonl");
  const donorLines = fs
    .readFileSync(donors[0].path, "utf8")
    .split(/\n/)
    .filter((line) => line.trim() && !line.includes('"kind":"workspace'))
    .slice(0, 20);
  fs.writeFileSync(fixture, `${donorLines.join("\n")}\n`, "utf8");

  const child = spawn(serverBin, ["server", "--transport", "stdio"], {
    cwd: repoRoot,
    stdio: ["pipe", "pipe", "pipe"],
    env: process.env,
  });
  const stderr: string[] = [];
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (c: string) => stderr.push(c));
  const conn = NativeAgentConnection.create({
    writeLine: (line) => child.stdin!.write(`${line}\n`),
    cwd: repoRoot,
  });
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (c: string) => conn.pushChunk(c));
  try {
    await conn.initialize({ name: "import-e2e", version: "0" });
    await conn.newSession();
    const result = await conn.importFromJsonl(fixture);
    assert.equal(result.cancelled, false);
    const after = await conn.getState();
    assert.ok(after.sessionFile);
    const lines = fs
      .readFileSync(String(after.sessionFile), "utf8")
      .trim()
      .split(/\n/)
      .filter(Boolean);
    assert.ok(
      lines.length > 1,
      `expected >1 lines, got ${lines.length}; stderr=${stderr.join("")}`,
    );
    const messages = await conn.getMessages();
    assert.ok(
      messages.length > 0,
      `expected imported messages in session/items/list, got ${messages.length}`,
    );
  } finally {
    await conn.dispose().catch(() => {});
    if (!child.killed) child.kill();
    fs.rmSync(fixtureDir, { recursive: true, force: true });
  }
});
