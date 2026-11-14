import assert from "node:assert/strict";
import test from "node:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { metricNames, parseTrace, positiveInteger, sessionResult, summarize, waitForObservation } from "./tui-latency-support.mjs";

const request = (id, method, sessionId) => ({ direction: "tui-to-server", id, method, payload: { params: { sessionId } } });
const response = (id, sessionId) => ({ direction: "server-to-tui", kind: "response", id, payload: { result: { session: { id: sessionId } } } });

test("only correlated successful RPCs identify the exact restored session", () => {
  const events = [request(1, "session/new"), response(1, "original"), request(2, "session/resume", "original")];
  assert.equal(sessionResult(events, "session/new"), "original");
  assert.equal(sessionResult(events, "session/resume", "original"), undefined);
  assert.equal(sessionResult([...events, response(3, "original")], "session/resume", "original"), undefined);
  assert.equal(sessionResult([...events, response(2, "other")], "session/resume", "original"), undefined);
  assert.equal(sessionResult([...events, { ...response(2, "original"), payload: { error: { message: "failed" } } }], "session/resume", "original"), undefined);
  assert.equal(sessionResult([...events, response(2, "original")], "session/resume", "original"), "original");
  assert.equal(sessionResult(events.slice(2), "session/new"), undefined, "old success cannot satisfy a new phase");
});

test("partial trace is ignored, but corrupt complete records fail", () => {
  assert.deepEqual(parseTrace('{"id":1}\n{"id":'), [{ id: 1 }]);
  assert.deepEqual(parseTrace(""), []);
  assert.throws(() => parseTrace("not-json\n"), SyntaxError);
});

test("statistics use nearest-rank quantiles with all raw samples", () => {
  const samples = [5, 1, 3, 2, 4].map(value => Object.fromEntries(metricNames.map(name => [name, value])));
  assert.deepEqual(summarize(samples), Object.fromEntries(metricNames.map(name => [name, { count: 5, min: 1, p50: 3, p95: 5, max: 5 }])));
  assert.throws(() => summarize([]), /empty/);
  for (const value of [undefined, NaN, Infinity, -1]) {
    assert.throws(() => summarize([{ ...samples[0], input_echo_ms: value }]), /Invalid metric/);
  }
});

test("sample/poll/timeout controls reject invalid integers", () => {
  assert.equal(positiveInteger("5", "samples"), 5);
  for (const value of [0, -1, 1.5, Infinity, "oops", ""]) assert.throws(() => positiveInteger(value, "samples"), /samples/);
});

test("observation waits for evidence, uses a monotonic clock, and fails on timeout or capture failure", async () => {
  let elapsed = 0;
  const base = { label: "input", now: () => elapsed, sleep: async ms => { elapsed += ms; }, pollMs: 25, timeoutMs: 100 };
  assert.deepEqual(await waitForObservation({ ...base, observe: () => ({ ready: elapsed >= 50 }), predicate: value => value.ready }), { ready: true });
  assert.equal(elapsed, 50);
  await assert.rejects(waitForObservation({ ...base, observe: () => false, predicate: Boolean }), /Timed out: input/);
  assert.equal(elapsed, 150);
  await assert.rejects(waitForObservation({ ...base, observe: () => { throw new Error("capture failed"); }, predicate: Boolean }), /capture failed/);
});

test("missing binary produces a failed report, not an invented baseline", t => {
  const directory = mkdtempSync(join(tmpdir(), "latency-report-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const output = join(directory, "report.json");
  const result = spawnSync(process.execPath, [fileURLToPath(new URL("./tui-latency.mjs", import.meta.url))], {
    env: { ...process.env, DEVO_BIN: join(directory, "nonexistent"), DEVO_BENCH_OUTPUT: output }, encoding: "utf8",
  });
  assert.equal(result.status, 1);
  const report = JSON.parse(readFileSync(output, "utf8"));
  assert.equal(report.status, "failed");
  assert.deepEqual(report.samples, []);
  assert.equal(report.summary_ms, undefined);
  assert.match(report.error, /Build this checkout first/);
});
