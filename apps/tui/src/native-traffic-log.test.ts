/**
 * Verifies the TUI native traffic log honours the shared Devo trace contract:
 * DEVO_PROTOCOL_TRACE gating, DEVO_HOME/traces placement, and the NDJSONL record
 * shape the desktop logger writes.
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  classifyNativeTrafficLine,
  createNativeTrafficLogFromEnv,
  formatProtocolTraceTimestamp,
  formatTracePreviewRecord,
  resolveNativeTrafficTracePath,
} from "./native-traffic-log.js";

const CLOCK = () => new Date("2026-09-16T01:02:03.000Z");
const PID = 4321;

function tempHome(): string {
  return mkdtempSync(path.join(tmpdir(), "devo-traffic-"));
}

test("stays disabled when DEVO_PROTOCOL_TRACE is unset or empty", () => {
  for (const env of [{}, { DEVO_PROTOCOL_TRACE: "" }, { DEVO_PROTOCOL_TRACE: "0" }]) {
    const log = createNativeTrafficLogFromEnv({ env, clock: CLOCK, pid: PID });
    assert.deepEqual(log.getState(), { enabled: false, path: null });
    log.record({ direction: "tui-to-server", kind: "request", method: "initialize" });
    assert.deepEqual(log.getState(), { enabled: false, path: null });
    assert.equal(resolveNativeTrafficTracePath({ env, clock: CLOCK, pid: PID }), null);
  }
});

test("writes NDJSONL records under DEVO_HOME/traces when enabled", () => {
  const home = tempHome();
  const log = createNativeTrafficLogFromEnv({
    env: { DEVO_HOME: home, DEVO_PROTOCOL_TRACE: "1" },
    clock: CLOCK,
    pid: PID,
  });

  const expected = path.join(
    home,
    "traces",
    `protocol-${PID}-${formatProtocolTraceTimestamp(CLOCK())}.ndjsonl`,
  );
  assert.deepEqual(log.getState(), { enabled: true, path: expected });
  assert.ok(existsSync(expected));

  log.record(classifyNativeTrafficLine("tui-to-server", '{"jsonrpc":"2.0","id":1,"method":"initialize"}'));
  log.record(classifyNativeTrafficLine("server-to-tui", '{"jsonrpc":"2.0","id":1,"result":{}}'));
  log.record(classifyNativeTrafficLine("server-to-tui", '{"jsonrpc":"2.0","method":"turn/started"}'));
  log.record(classifyNativeTrafficLine("tui-to-server", "not json"));

  const records = readFileSync(expected, "utf-8")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>);

  assert.deepEqual(
    records.map((record) => ({
      timestamp: record.timestamp,
      direction: record.direction,
      kind: record.kind,
      id: record.id,
      method: record.method,
    })),
    [
      {
        timestamp: "2026-09-16T01:02:03.000Z",
        direction: "tui-to-server",
        kind: "request",
        id: 1,
        method: "initialize",
      },
      {
        timestamp: "2026-09-16T01:02:03.000Z",
        direction: "server-to-tui",
        kind: "response",
        id: 1,
        method: undefined,
      },
      {
        timestamp: "2026-09-16T01:02:03.000Z",
        direction: "server-to-tui",
        kind: "notification",
        id: undefined,
        method: "turn/started",
      },
      {
        timestamp: "2026-09-16T01:02:03.000Z",
        direction: "tui-to-server",
        kind: "invalid",
        id: undefined,
        method: undefined,
      },
    ],
  );
});

test("honours DEVO_PROTOCOL_TRACE_FILE and falls back when DEVO_HOME is invalid", () => {
  const dir = tempHome();
  const explicit = path.join(dir, "nested", "my-trace.ndjsonl");
  const explicitLog = createNativeTrafficLogFromEnv({
    env: { DEVO_PROTOCOL_TRACE: "true", DEVO_PROTOCOL_TRACE_FILE: explicit },
    clock: CLOCK,
    pid: PID,
  });
  assert.deepEqual(explicitLog.getState(), { enabled: true, path: explicit });
  assert.ok(existsSync(explicit));

  const fileHome = path.join(dir, "not-a-dir");
  const fallback = resolveNativeTrafficTracePath({
    env: { DEVO_HOME: fileHome, DEVO_PROTOCOL_TRACE: "1" },
    clock: CLOCK,
    pid: PID,
  });
  assert.ok(fallback);
  assert.match(fallback!, /devo-traces[\\/]protocol-4321-/);
});

test("/traces on enables at runtime and off stops recording", () => {
  const home = tempHome();
  const log = createNativeTrafficLogFromEnv({
    env: { DEVO_HOME: home },
    clock: CLOCK,
    pid: PID,
  });
  assert.equal(log.getState().enabled, false);

  const enabled = log.enable();
  assert.equal(enabled.enabled, true);
  assert.equal(enabled.path, path.join(
    home,
    "traces",
    `protocol-${PID}-${formatProtocolTraceTimestamp(CLOCK())}.ndjsonl`,
  ));

  log.record({ direction: "tui-to-server", kind: "request", method: "session/new" });
  assert.equal(log.preview(10).length, 1);

  log.disable();
  assert.deepEqual(log.getState(), { enabled: false, path: null });
  log.record({ direction: "tui-to-server", kind: "request", method: "session/list" });
  assert.equal(log.preview(10).length, 1, "disabled log must not append");

  const reenabled = log.enable();
  assert.equal(reenabled.enabled, true);
});

test("preview returns the newest records and classifies frames", () => {
  const home = tempHome();
  const log = createNativeTrafficLogFromEnv({
    env: { DEVO_HOME: home, DEVO_PROTOCOL_TRACE: "1" },
    clock: CLOCK,
    pid: PID,
  });
  for (let index = 1; index <= 3; index += 1) {
    log.record({ direction: "tui-to-server", kind: "request", id: index, method: "ping" });
  }
  assert.deepEqual(
    log.preview(2).map((record) => record.id),
    [2, 3],
  );
  assert.equal(log.list().length, 1);

  assert.equal(
    formatTracePreviewRecord(
      { direction: "server-to-tui", kind: "response", method: "initialize", id: 1, payload: {} },
      0,
    ),
    "  1 <- response initialize id=1 2B",
  );
});
