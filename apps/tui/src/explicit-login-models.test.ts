import assert from "node:assert/strict";
import { test } from "node:test";
import { ExplicitLoginModels } from "./explicit-login-models.js";
import { ProviderAuthFlows } from "../lib/coding-agent/src/modes/interactive/auth-flows.js";

test("explicit provider authentication awaits connection setup before catalog refresh and success", async () => {
  const calls: unknown[] = [];
  const flows = new ProviderAuthFlows({
    ui: {} as never,
    modelRegistry: { refresh: () => calls.push("registry") } as never,
    getAvailableModels: async () => [],
    showStatus: () => calls.push("success"),
    showError: () => calls.push("error"),
    onProviderAuthenticated: async (...args) => {
      calls.push(args);
      await Promise.resolve();
      calls.push("configured");
    },
    onAuthChanged: async () => { calls.push("catalog"); },
    onLoginCompleted: () => { calls.push("completed"); },
  });
  const complete = flows as unknown as { completeProviderAuthentication(...args: unknown[]): Promise<unknown> };
  await complete.completeProviderAuthentication("openai-codex", "Codex", "oauth", undefined, "provider", "/test/auth");
  assert.deepEqual(calls, ["registry", ["openai-codex", "oauth", "provider"], "configured", "catalog", "success", "completed"]);
});

test("empty discovery surfaces a selection blocker and cannot write configuration", async () => {
  const calls: unknown[] = [];
  const models = new ExplicitLoginModels({ request: async (method, params) => {
    calls.push({ method, params });
    return { models: {} };
  } });
  await assert.rejects(models.discover("openai-codex"), /no models were returned/);
  await models.pinSelected("openai-codex", "missing");
  assert.deepEqual(models.models(), []);
  assert.deepEqual(calls, [{ method: "provider/discover", params: { providerId: "openai-codex", forceRefresh: true } }]);
});

test("failed selected model save blocks session-selection caller", async () => {
  const models = new ExplicitLoginModels({ request: async (method) => {
    if (method === "provider/discover") return { models: { chosen: {} } };
    if (method === "provider/list") return { providers: [{ id: "custom", wireApis: ["responses"], baseUrl: "https://user.example" }] };
    throw new Error("save failed");
  } });
  await models.discover("custom");
  await assert.rejects(models.pinSelected("custom", "chosen"), /save failed/);
});

test("pinning drops stale selection suggestions and never overwrites an already saved model", async () => {
  const writes: unknown[] = [];
  let alreadySaved = false;
  const models = new ExplicitLoginModels({ request: async (method, params) => {
    if (method === "provider/discover") return { models: { chosen: { baseUrl: "https://discovered.example" }, other: {} } };
    if (method === "provider/list") return {
      providers: [{ id: "custom", wireApis: ["openai_responses"], baseUrl: "https://pinned.example" }],
      connectionModels: alreadySaved ? { custom: { chosen: { baseUrl: "https://authored.example" } } } : {},
    };
    writes.push(params);
    return {};
  } });
  await models.discover("custom");
  await models.pinSelected("custom", "chosen");
  await models.pinSelected("custom", "chosen");
  assert.equal(writes.length, 1);
  assert.deepEqual(models.models().map(m => m.id), ["other"]);
  alreadySaved = true;
  await models.discover("custom");
  await models.pinSelected("custom", "chosen");
  assert.equal(writes.length, 1);
  assert.deepEqual(models.models().map(m => m.id), ["other"]);
  models.clear("custom");
  assert.deepEqual(models.models(), []);
});

test("pin preserves provider settings while excluding unrelated models and discovered metadata", async () => {
  const connection = {
    id: "custom", name: "My provider", description: "Private gateway",
    baseUrl: "https://pinned.example", wireApis: ["openai_responses"], enabled: true,
    headers: { "X-Project": "my-project" }, credential: "stored-credential",
    options: { region: "local" }, request: { timeout: 42 }, compat: { responses: true },
    models: { unrelated: { name: "Do not resend", capabilities: { tools: true } } },
    modelOverrides: { unrelated: { name: "Authored label" } },
  };
  const writes: unknown[] = [];
  const models = new ExplicitLoginModels({ request: async (method, params) => {
    if (method === "provider/discover") return { models: {
      chosen: { name: "Live name", cost: { input: 7 }, capabilities: { tools: true }, contextWindow: 1234 },
      other: { name: "Not selected" },
    } };
    if (method === "provider/list") return { providers: [connection] };
    writes.push(params);
    return {};
  } });
  await models.discover("custom");
  await models.pinSelected("custom", "chosen");
  assert.deepEqual(writes, [{ provider: {
    ...connection,
    models: { chosen: { origin: "user", baseUrl: "https://pinned.example", wireApi: "openai_responses", requestModel: "chosen" } },
    modelOverrides: {},
  } }]);
});
