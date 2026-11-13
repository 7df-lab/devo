/** Test-only delay/failure at the schedule RPC boundary; real product keyboard/UI remain unchanged. */
import { ScheduledManagerComponent } from "../lib/coding-agent/src/modes/interactive/components/scheduled-manager.ts";
import { NativeAgentConnection } from "../src/native-agent-connection.ts";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
const root = process.env.DEVO_CLIPBOARD_SMOKE_DIR;
if (!root) throw new Error("scheduled pending smoke needs control directory");
let inputs = 0;
const originalInput = ScheduledManagerComponent.prototype.handleInput;
ScheduledManagerComponent.prototype.handleInput = function (data) {
  const result = originalInput.call(this, data);
  writeFileSync(join(root,"inputs"),String(++inputs));
  return result;
};
const original = NativeAgentConnection.prototype.manageScheduledTask;
let count = 0;
NativeAgentConnection.prototype.manageScheduledTask = async function (...args) {
  const n = ++count;
  writeFileSync(join(root,"calls"),String(count));
  writeFileSync(join(root,`action-${n}.started`),"started");
  const end = Date.now()+60000;
  while (!existsSync(join(root,`action-${n}.release`))) {
    if (Date.now()>end) throw new Error("schedule test gate timed out");
    await sleep(20);
  }
  if (readFileSync(join(root,`action-${n}.release`),"utf8") === "fail") throw new Error("controlled schedule failure");
  return original.apply(this,args);
};
writeFileSync(join(root,"preload.ready"),"ready");
