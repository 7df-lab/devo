import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { getModel, getModels, getProviders } from "../lib/ai/src/models.js";
import { createDevoUiServices } from "./host.js";

test("legacy model helpers expose no bundled directory or fallback", () => {
  assert.deepEqual({
    providers: getProviders(),
    models: getModels("openai"),
    model: getModel("openai", "gpt-4o"),
  }, { providers: [], models: [], model: undefined });
});

test("Native host preserves but never loads legacy model files", () => {
  const home = mkdtempSync(join(tmpdir(), "devo-model-sources-"));
  const legacy = JSON.stringify({ providers: { legacy: {
    baseUrl: "https://example.invalid/v1", api: "openai-completions",
    models: [{ id: "legacy-model" }],
  } } });
  const snapshot = JSON.stringify({ data: [{ id: "snapshot-model" }] });
  try {
    writeFileSync(join(home, "models.json"), legacy);
    writeFileSync(join(home, "prime-inference-models-cache.json"), snapshot);
    const services = createDevoUiServices({ home, cwd: home });
    services.modelRegistry.refresh();
    assert.deepEqual({
      models: services.modelRegistry.getAll(),
      error: services.modelRegistry.getError(),
      legacy: readFileSync(join(home, "models.json"), "utf8"),
      snapshot: readFileSync(join(home, "prime-inference-models-cache.json"), "utf8"),
    }, { models: [], error: undefined, legacy, snapshot });
    assert.ok(services.modelRegistry.authStorage === services.authStorage);
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});
