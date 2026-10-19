import assert from "node:assert/strict";
import { test } from "node:test";
import { ModelCatalogRefresh } from "./model-catalog-refresh.js";
import { NativeAgentConnection } from "./native-agent-connection.js";

test("background checks coalesce, respect cooldown and retry when due", async () => {
  let now = 0;
  const calls: unknown[] = [];
  let complete!: (result: unknown) => void;
  const refresh = new ModelCatalogRefresh({ request: async (method, params) => {
    calls.push({ method, params });
    return new Promise((resolve) => { complete = resolve; });
  } }, () => now);
  const first = refresh.refresh();
  assert.equal(refresh.refresh(), first);
  complete({ status: "offline" });
  await first;
  await refresh.refresh();
  assert.deepEqual(calls, [{ method: "model/catalog/refresh", params: { policy: "ifStale" } }]);
  now = 60_000;
  const second = refresh.refresh();
  complete({ status: "offline" });
  await second;
  assert.equal(calls.length, 2);
});

test("refresh updates authenticated Codex models without forcing network requests", async () => {
  const calls: unknown[] = [];
  const refresh = new ModelCatalogRefresh({ request: async (method, params) => {
    calls.push({ method, params });
    return method === "provider/list" ? { connectedProviderIds: ["openai-codex", "custom"] } : { status: "cached" };
  } });
  await refresh.refresh();
  assert.deepEqual(calls, [
    { method: "model/catalog/refresh", params: { policy: "ifStale" } },
    { method: "provider/list", params: {} },
    { method: "provider/discover", params: { providerId: "openai-codex", forceRefresh: false } },
  ]);
});

test("failed requests are silent, back off and can recover", async () => {
  let now = 1;
  let attempts = 0;
  const refresh = new ModelCatalogRefresh({ request: async () => {
    attempts++;
    if (attempts === 1) throw new Error("offline");
    return { status: "offline" };
  } }, () => now);
  await refresh.refresh();
  await refresh.refresh();
  assert.equal(attempts, 1);
  now += 60_000;
  await refresh.refresh();
  assert.equal(attempts, 2);
});

test("Native cached reads stay immediate while a remote update is pending", async () => {
  let release!: () => void;
  let currentModel = "cached-model";
  let connection: NativeAgentConnection;
  const respond = (id: number, result: unknown) => connection.pushChunk(JSON.stringify({ id, result }) + "\n");
  connection = NativeAgentConnection.create({ writeLine: (line) => {
    const { id, method } = JSON.parse(line);
    if (method === "model/catalog/refresh") {
      release = () => { currentModel = "fresh-model"; respond(id, { status: "updated" }); };
    } else if (method === "model/list") {
      respond(id, { models: [{ provider: "openai", id: currentModel, name: currentModel }] });
    } else if (method === "provider/list") {
      respond(id, { providers: [], connectedProviderIds: ["openai"] });
    } else if (method === "credential/list") {
      respond(id, { credentials: [] });
    } else {
      throw new Error(`Unexpected request: ${method}`);
    }
  } });
  const cached = await connection.getModelCatalog();
  const updating = connection.refreshModelCatalog();
  assert.deepEqual(await connection.getModelCatalog(), cached);
  release();
  const fresh = await updating;
  assert.deepEqual(fresh.models.map(({ provider, id }) => ({ provider, id })), [{ provider: "openai", id: "fresh-model" }]);
  assert.deepEqual(fresh.configuredProviders, ["openai"]);
});
