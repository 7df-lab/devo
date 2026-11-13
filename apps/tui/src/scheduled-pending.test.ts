import assert from "node:assert/strict";
import { test } from "node:test";
import { stripVTControlCharacters } from "node:util";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import { ScheduledManagerComponent, type ScheduledManagerActionState } from "../lib/coding-agent/src/modes/interactive/components/scheduled-manager.js";

const flush = () => new Promise<void>(resolve => setImmediate(resolve));
const plain = (panel: ScheduledManagerComponent, width = 100) => stripVTControlCharacters(panel.render(width).join("\n"));
for (const fails of [false, true]) test(`pending operation survives close and reopen (${fails ? "failure" : "success"})`, async () => {
  initTheme("dark");
  const state: ScheduledManagerActionState = {};
  let resolve!: () => void; let reject!: (error: Error) => void; let calls = 0; let closes = 0;
  const job = { id: "one", sessionId: "s", status: "active", source: "heartbeat", schedule: { expression: "every 1h" }, prompt: "Check progress", runCount: 0, createdAt: "2026-01-01" } as any;
  const make = () => new ScheduledManagerComponent({ actionState: state, getScheduledTasks: () => [{ job }], getRows: () => 24,
    onAction: () => { calls++; return new Promise<void>((a,b) => { resolve=a; reject=b; }); }, onClose: () => { closes++; }, requestRender: () => {} });
  const first = make(); first.handleInput("\r"); first.handleInput("\r");
  for (const width of [40,100]) assert.match(plain(first,width), /Pausing scheduled task/);
  first.handleInput("\r"); first.handleInput("\x1b[B"); first.handleInput("\r"); assert.equal(calls,1);
  first.handleInput("\x1b"); assert.equal(closes,1);
  const second = make(); assert.match(plain(second), /Pausing scheduled task/);
  second.handleInput("\r"); assert.equal(calls,1);
  if (fails) reject(new Error("offline")); else { job.status="paused"; resolve(); }
  await flush();
  assert.equal(state.pending,undefined);
  // The old panel must not navigate the new one into its detail/action mode.
  assert.match(plain(second), /Enter details/);
  if (fails) assert.match(plain(second), /Pausing scheduled task failed: offline/);
  else assert.match(plain(second), /paused/);
  second.handleInput("\r"); second.handleInput("\r"); assert.equal(calls,2); resolve(); await flush();
});
