import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { startResizeProvider } from "../scripts/resize-status-smoke.mjs";

test("resize provider associates held turns and isolates concurrent title requests", async () => {
  const home = await mkdtemp(join(tmpdir(), "resize-provider-"));
  const provider = await startResizeProvider(home);
  try {
    const config = JSON.parse(await readFile(join(home, "providers.json"), "utf8"));
    const url = config.provider.deepseek.base_url;
    const post = (body: unknown) => fetch(url, { method: "POST", body: JSON.stringify(body) });
    const turn0 = await post({ stream: true, messages: [{ role: "user", content: "resize status test 0" }] });
    const turn1 = await post({ stream: true, messages: [{ role: "user", content: [{ type: "text", text: "resize status test 1" }] }] });
    const title = await post({ stream: false, messages: [{ role: "user", content: "title for resize status test 0" }] });
    assert.match(title.headers.get("content-type")!, /application\/json/);
    assert.equal((await title.json()).choices[0].message.content, "Resize smoke");
    assert.equal(provider.ready(0), true);
    assert.equal(provider.ready(1), true);
    provider.release(1);
    assert.match(await turn1.text(), /Resize acknowledged/);
    assert.equal(provider.ready(0), true);
    assert.equal(provider.ready(1), false);
    const duplicate = await post({ stream: true, messages: [{ role: "user", content: "resize status test 0" }] });
    assert.equal(duplicate.status, 400);
    provider.release(0);
    assert.match(await turn0.text(), /Resize acknowledged/);
    assert.equal(provider.ready(0), false);
    assert.throws(() => provider.release(0), /No held response/);
  } finally {
    provider.close();
    await rm(home, { recursive: true, force: true });
  }
});
