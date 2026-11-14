import assert from "node:assert/strict";
import { test } from "node:test";
import { ProviderLogoutState } from "./provider-logout.js";
import { ProviderAuthFlows } from "../lib/coding-agent/src/modes/interactive/auth-flows.js";
import { AuthStorage } from "../lib/coding-agent/src/core/auth-storage.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

initTheme("dark");
test("missing pinned credentials are not configured; local and unrelated routes survive", () => {
  const state = new ProviderLogoutState();
  state.reconcile([{ id: "remote", credential: "remote-key" }, { id: "other", credential: "other-key" }, { id: "local" }], [{ id: "other-key" }]);
  assert.deepEqual([state.unavailable("remote"), state.unavailable("other"), state.unavailable("local")], [true, false, false]);
  state.logout("other");
  state.reconcile([], [{ id: "other-key" }]);
  assert.equal(state.unavailable("other"), true);
  state.login("other");
  assert.equal(state.unavailable("other"), false);
});

for (const fail of [false, true]) test(`logout waits for Native revocation before ${fail ? "error" : "success"}`, async () => {
  const auth = AuthStorage.inMemory();
  auth.set("deepseek", { type: "api_key", key: "test-only" });
  const calls: string[] = [];
  let overlay: { handleInput(data: string): void } | undefined;
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => { finish = resolve; });
  const flows = new ProviderAuthFlows({
    ui: { terminal: { rows: 30 }, requestRender() {}, showOverlay(component: never) { overlay = component; return { hide() {} }; } } as never,
    modelRegistry: { authStorage: auth, refresh() { calls.push("refresh"); }, getProviderDisplayName(id: string) { return id; } } as never,
    getAvailableModels: async () => [],
    onProviderLoggedOut: async (provider) => { assert.equal(auth.has(provider), false); calls.push("delete"); await pending; if (fail) throw new Error("delete failed"); },
    onAuthChanged: () => { calls.push("catalog"); },
    showStatus: () => calls.push("success"),
    showError: () => calls.push("error"),
  });
  const result = flows.runLogout();
  await Promise.resolve();
  overlay!.handleInput("\r");
  await Promise.resolve();
  assert.deepEqual(calls, ["delete"]);
  finish();
  assert.equal(await result, fail ? null : "deepseek");
  assert.deepEqual(calls, fail ? ["delete", "catalog", "error"] : ["delete", "refresh", "catalog", "success"]);
});

test("legacy credential bindings match Native migration without cross-provider fallback", () => {
  const state = new ProviderLogoutState();
  state.reconcile([{ id: "openrouter", credential: "openrouter_api_key" }, { id: "openai", credential: "openai_oauth" }, { id: "other", credential: "missing" }], [{ id: "openrouter" }, { id: "openai" }]);
  assert.equal(state.unavailable("openrouter"), false);
  assert.equal(state.unavailable("openai"), false);
  assert.equal(state.unavailable("other"), true);
});
