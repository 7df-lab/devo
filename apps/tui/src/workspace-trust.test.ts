import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, readdir, readFile, writeFile, rm, symlink, realpath } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { PassThrough } from "node:stream";
import { WorkspaceTrust, promptWorkspaceTrust, requireSessionWorkspace } from "./workspace-trust.js";
import { StdioJsonRpc } from "./stdio-jsonrpc.js";
async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await mkdtemp(join(tmpdir(), "devo-trust-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const home = join(root, "home"), workspace = join(root, "project");
  await mkdir(home); await mkdir(workspace);
  return { root, home, workspace };
}
test("explicit acceptance is remembered with exact-directory scope", async (t) => {
  const { home, workspace } = await fixture(t); let prompts = 0;
  await new WorkspaceTrust(home, async () => { prompts++; return true; }).require(workspace);
  const trust = new WorkspaceTrust(home, async () => { prompts++; return false; });
  await trust.require(workspace);
  const child = join(workspace, "child"); await mkdir(child);
  await assert.rejects(trust.require(child), /not trusted/); assert.equal(prompts, 2);
});
test("decline and corrupt records never confer trust", async (t) => {
  const { home, workspace } = await fixture(t);
  await assert.rejects(new WorkspaceTrust(home, async () => false).require(workspace), /not trusted/);
  await new WorkspaceTrust(home, async () => true).require(workspace);
  const dir = join(home, "workspace-trust");
  await writeFile(join(dir, (await readdir(dir))[0]), '{"version":1,"trusted":true}');
  await assert.rejects(new WorkspaceTrust(home, async () => false).require(workspace), /not trusted/);
});
test("home inside workspace cannot supply persisted consent", async (t) => {
  const { workspace } = await fixture(t); const home = join(workspace, ".devo"); await mkdir(home);
  const accepted = new WorkspaceTrust(home, async () => true);
  await accepted.require(workspace); accepted.finishStartup(); await accepted.require(workspace);
  assert.deepEqual(await readdir(home), []);
  await assert.rejects(new WorkspaceTrust(home, async () => false).require(workspace), /not trusted/);
});
test("runtime switches fail closed without a competing stdin reader", async (t) => {
  const { home, workspace } = await fixture(t);
  const trust = new WorkspaceTrust(home, async () => assert.fail("must not prompt")); trust.finishStartup();
  await assert.rejects(trust.require(workspace), /start devo in that directory/);
});
test("concurrent workspace acceptances do not overwrite each other", async (t) => {
  const { root, home, workspace } = await fixture(t); const second = join(root, "second"); await mkdir(second);
  await Promise.all([workspace, second].map(cwd => new WorkspaceTrust(home, async () => true).require(cwd)));
  assert.equal((await readdir(join(home, "workspace-trust"))).length, 2);
  const trusted = new WorkspaceTrust(home, async () => assert.fail("saved trust missing"));
  await trusted.require(workspace); await trusted.require(second);
});
test("symlinks share canonical trust, retargets do not", { skip: process.platform === "win32" }, async (t) => {
  const { root, home, workspace } = await fixture(t);
  const alias = join(root, "alias"), second = join(root, "second"); await mkdir(second); await symlink(workspace, alias);
  const trust = new WorkspaceTrust(home, async () => true);
  await trust.require(alias); trust.finishStartup(); await trust.require(workspace);
  await rm(alias); await symlink(second, alias);
  await assert.rejects(trust.require(alias), /not trusted/);
});
for (const [answer, expected] of [["yes\n", true], ["\n", false], ["y\n", false], ["no\n", false], ["", false]] as const) {
  test(`prompt requires explicit yes: ${JSON.stringify(answer)}`, async () => {
    const input = new PassThrough(), output = new PassThrough();
    const result = promptWorkspaceTrust("/project\u001b[2J", input, output); input.end(answer);
    assert.equal(await result, expected); assert.match(output.read().toString(), /\\u001b/);
  });
}
test("session admission only reads metadata when trust is denied", async (t) => {
  const { home, workspace } = await fixture(t); const methods: string[] = [];
  const rpc = new StdioJsonRpc(line => {
    const request = JSON.parse(line); methods.push(request.method);
    queueMicrotask(() => rpc.handleIncomingLine(JSON.stringify({ id: request.id, result: { session: { cwd: workspace } } })));
  });
  await assert.rejects(requireSessionWorkspace(rpc, new WorkspaceTrust(home, async () => false), "old"), /not trusted/);
  assert.deepEqual(methods, ["session/read"]);
});
test("product entry gates workspace before spawning server", async () => {
  const source = await readFile(new URL("./index.ts", import.meta.url), "utf8");
  assert.ok(source.indexOf("await trust.require(process.cwd())") < source.indexOf("const child = spawn("));
  assert.ok(source.includes("workspaceTrust: trust"));
});

test("denied Native switch/new/import leaves current UI valid and sends no activating RPC", async (t) => {
  const { NativeAgentConnection } = await import("./native-agent-connection.js");
  const { home, workspace, root } = await fixture(t);
  const other = join(root, "other"); await mkdir(other);
  const trust = new WorkspaceTrust(home, async () => true);
  await trust.require(workspace); trust.finishStartup();
  const methods: string[] = [];
  let conn: ReturnType<typeof NativeAgentConnection.create>;
  conn = NativeAgentConnection.create({ cwd: workspace, workspaceTrust: trust, writeLine(line) {
    const request = JSON.parse(line); if (request.id == null) return; methods.push(request.method);
    const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ id: request.id, result }) + "\n");
    if (request.method === "session/read") return respond({ session: { cwd: request.params.sessionId === "A" ? workspace : other } });
    if (request.method === "session/resume") return respond({ session: { id: "A", cwd: workspace } });
    if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
    respond({ data: [], agents: [] });
  } });
  t.after(() => conn.dispose());
  await conn.switchSession("A", { historyLoadMode: "deferred" });
  let invalidated = 0; conn.onBeforeSessionInvalidate(() => { invalidated++; });
  methods.length = 0;
  await assert.rejects(conn.switchSession("B"), /not trusted/);
  await assert.rejects(conn.watchSession("B"), /not trusted/);
  await assert.rejects(conn.importFromJsonl("missing.jsonl", other), /not trusted/);
  assert.equal(invalidated, 0);
  assert.equal((await conn.getState()).sessionId, "A");
  assert.ok(!methods.includes("session/resume") && !methods.includes("session/import"));
  const blocked = NativeAgentConnection.create({ cwd: other, workspaceTrust: trust, writeLine() { assert.fail("new must not send"); } });
  blocked.onBeforeSessionInvalidate(() => { invalidated++; });
  await assert.rejects(blocked.newSession(), /not trusted/);
  assert.equal(invalidated, 0);
});

test("retargeting a symlink while the prompt is open rejects acceptance", { skip: process.platform === "win32" }, async (t) => {
  const { root, home, workspace } = await fixture(t);
  const alias = join(root, "alias"), other = join(root, "other"); await mkdir(other); await symlink(workspace, alias);
  const trust = new WorkspaceTrust(home, async () => {
    await rm(alias); await symlink(other, alias); return true;
  });
  await assert.rejects(trust.require(alias), /Workspace changed/);
  assert.deepEqual(await readdir(home), []);
});

for (const operation of ["new", "switch", "import"] as const) {
  for (const invalidate of ["newer-switch", "dispose"] as const) {
    test(`late ${operation} admission cannot activate after ${invalidate}`, { timeout: 10_000 }, async (t) => {
      const { NativeAgentConnection } = await import("./native-agent-connection.js");
      const { home, workspace, root } = await fixture(t);
      const other = join(root, "other"); await mkdir(other);
      const canonicalOther = await realpath(other);
      let approve!: (value: boolean) => void;
      const trust = new WorkspaceTrust(home, async cwd => cwd === canonicalOther ? true : new Promise(resolve => { approve = resolve; }));
      const activations: string[] = [];
      let conn: ReturnType<typeof NativeAgentConnection.create>;
      conn = NativeAgentConnection.create({ cwd: workspace, workspaceTrust: trust, writeLine(line) {
        const request = JSON.parse(line); if (request.id == null) return;
        const respond = (result: unknown) => conn.pushChunk(JSON.stringify({ id: request.id, result }) + "\n");
        if (request.method === "session/read") return respond({ session: { cwd: request.params.sessionId === "B" ? other : workspace } });
        if (request.method === "session/resume") { activations.push(request.params.sessionId); return respond({ session: { id: request.params.sessionId, cwd: other } }); }
        if (request.method === "session/new" || request.method === "session/import") activations.push(request.method);
        if (request.method === "subscription/create") return respond({ subscriptionId: "sub", cursors: [] });
        respond({ data: [], agents: [] });
      } });
      t.after(() => conn.dispose());
      const pending = operation === "new" ? conn.newSession() : operation === "switch" ? conn.switchSession("A") : conn.importFromJsonl("missing.jsonl");
      while (!approve) await new Promise(resolve => setImmediate(resolve));
      if (invalidate === "dispose") await conn.dispose(); else await conn.switchSession("B", { historyLoadMode: "deferred" });
      approve(true);
      assert.deepEqual(await pending, { cancelled: true });
      assert.deepEqual(activations, invalidate === "dispose" ? [] : ["B"]);
    });
  }
}
