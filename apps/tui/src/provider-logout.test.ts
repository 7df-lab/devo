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
  let overlay: { handleInput(data: string): void; render(width: number): string[] } | undefined;
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
  const rendered = overlay!.render(100).join("\n");
  assert.match(rendered, /Saved Credentials/);
  assert.match(rendered, /Choose a credential to remove\./);
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

test("logout smoke reselects the exact provider among same-name catalog models", async () => {
  const {ModelSelectorComponent} = await import("../lib/coding-agent/src/modes/interactive/components/model-selector.js");
  const {ModelRegistry} = await import("../lib/coding-agent/src/core/model-registry.js");
  const {normalizeAgentModel} = await import("./native-agent-connection.js");
  const models = ["agentrouter", "aimlapi", "deepseek"].map(provider => ({
    ...normalizeAgentModel({provider,id:"deepseek-v4-flash",name:"DeepSeek V4 Flash",reasoning:true})!,
    // Preserve catalog IDs as supplied to the picker, including prefix collisions.
    id:provider === "deepseek" ? "deepseek-v4-flash" : "deepseek/deepseek-v4-flash",
  }));
  let selected: string | undefined;
  const selector = new ModelSelectorComponent(
    {requestRender(){}} as never, undefined, ModelRegistry.inMemory(AuthStorage.inMemory()), [],
    model => {selected=model.provider;}, () => {}, "deepseek/deepseek-v4-flash",
    {availableModels:models as never,configuredProviders:new Set(),inline:true,getRows:()=>30},
  );
  assert.match(selector.render(100).join("\n"),/require sign in/);
  // Mirror the smoke's visible-row identity check rather than assuming rank.
  let navigations = 0;
  while (true) {
    const viewport = selector.render(140).join("\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
    const row = viewport.split("\n").find(line => /^\s*›.*require sign in · /.test(line));
    assert.ok(row, "selected provider row is visible");
    if (/require sign in · deepseek\s*$/.test(row)) break;
    assert.ok(++navigations < models.length, "exact provider must be reachable");
    selector.handleInput("\x1b[B");
  }
  assert.equal(navigations, 2);
  selector.handleInput("\r");
  assert.equal(selected,"deepseek");
});

test("Native-only logged-out model opens the actual API key dialog and cancellation stays unauthenticated", async () => {
  const { InteractiveMode } = await import("../lib/coding-agent/src/modes/interactive/interactive-mode.js");
  const auth = AuthStorage.inMemory();
  let overlay: {handleInput(data:string):void; render(width:number):string[]} | undefined;
  const errors: string[] = [];
  const mode = Object.create(InteractiveMode.prototype) as {
    ui: unknown; modelRegistry: unknown; options: unknown;
    connectionModelCatalog: Array<{provider:string;id:string}>;
    connectionConfiguredProviders: Set<string>;
    showError(message:string):void;
    createAuthFlows(): ProviderAuthFlows;
    ensureModelProviderConfigured(model: unknown, flows: ProviderAuthFlows): Promise<boolean>;
  };
  mode.ui = {terminal:{rows:30},requestRender(){},showOverlay(component:never){overlay=component;return {hide(){}};}};
  Object.defineProperty(mode, "modelRegistry", {value:{authStorage:auth,getAll:()=>[],getProviderDisplayName:(id:string)=>id}});
  mode.options = {excludedLoginProviderIds:["excluded-native"]};
  mode.connectionModelCatalog = [];
  mode.connectionConfiguredProviders = new Set();
  mode.showError = message => errors.push(message);
  const flows = mode.createAuthFlows();
  assert.equal(flows.getLoginProviderOptions().some(p=>p.id === "deepseek"),false);
  // Simulate a catalog arriving after the menu/auth flow was created.
  mode.connectionModelCatalog = [{provider:"deepseek",id:"deepseek-v4-flash"},{provider:"excluded-native",id:"hidden"}];
  const options = flows.getLoginProviderOptions();
  assert.equal(options.find(p=>p.id === "deepseek")?.authType,"api_key");
  assert.equal(options.some(p=>p.id === "excluded-native"),false);
  assert.equal(options.find(p=>p.id === "serper")?.category,"service");
  const result = mode.ensureModelProviderConfigured(mode.connectionModelCatalog[0], flows);
  assert.match(overlay!.render(100).join("\n"), /Enter API key:/);
  overlay!.handleInput("\x1b");
  assert.equal(await result,false);
  assert.equal(auth.has("deepseek"),false);
  assert.deepEqual(errors,[]);
});
