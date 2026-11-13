import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
// @ts-ignore JavaScript smoke provider is also exercised through HTTP here.
import { startSubagentRosterProvider } from "../scripts/subagent-roster-smoke.mjs";

test("subagent smoke holds child and parent independently and excludes title calls", async () => {
  const home = await mkdtemp(join(tmpdir(), "roster-provider-"));
  const provider = await startSubagentRosterProvider(home);
  try {
    const config = JSON.parse(await readFile(join(home, "providers.json"), "utf8"));
    const url = config.provider.deepseek.base_url;
    const request = (text: string, stream = true) => fetch(url, {
      method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ stream, messages: [{ role: "user", content: text }] }),
    });
    assert.match(await (await request("ROSTER_ROOT_TASK")).text(), /rlm.spawn/);
    const child = await request("ROSTER_CHILD_TASK");
    const parent = await request("ROSTER_ROOT_TASK");
    const title = await request("title", false);
    assert.equal((await title.json()).choices[0].finish_reason, "stop");
    assert.equal(provider.ready("child"), true);
    assert.equal(provider.ready("parent"), true);
    provider.release("child");
    assert.match(await child.text(), /ROSTER_CHILD_FINISHED/);
    assert.equal(provider.ready("parent"), true);
    provider.release("parent");
    assert.match(await parent.text(), /ROSTER_PARENT_FINISHED/);
  } finally { provider.close(); await rm(home, { recursive: true, force: true }); }
});
