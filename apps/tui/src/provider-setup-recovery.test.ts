import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { ConfigurationMenuComponent } from "../lib/coding-agent/src/modes/interactive/components/configuration-menu.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";
import { getKeybindings, KeybindingsManager, setKeybindings } from "@earendil-works/pi-tui";
import { KEYBINDINGS } from "../lib/coding-agent/src/core/keybindings.js";

test("cancelled empty-model onboarding can reopen provider setup and model selection locally", async () => {
  const tabs: string[] = [];
  const editor = {
    onSubmit: undefined as ((text: string) => Promise<void>) | undefined,
    getText: () => "",
    setText: () => {},
  };
  let shown = false;
  const mode = Object.create(InteractiveMode.prototype) as {
    runStartupOnboarding(): Promise<boolean>;
    setupEditorSubmitHandler(): void;
  };
  Object.assign(mode, {
    options: { excludedLoginProviderIds: ["prime-inference"] },
    uiServices: {
      settingsManager: {
        getOnboardingShown: () => shown,
        setOnboardingShown: (value: boolean) => { shown = value; },
        flush: async () => {},
      },
      modelRegistry: { refresh: () => {}, hasConfiguredAuth: () => false },
    },
    getCurrentModel: () => undefined,
    getModelCandidates: async () => [],
    // Resolve without selecting a model, as closing the configuration menu does.
    showConfigurationMenu: async (tab: string) => { tabs.push(tab); },
    defaultEditor: editor,
    editor,
    clearShortcutGuide: () => {},
    inputSubmissionGeneration: 0,
    inputSubmissionsPending: 0,
    pendingPromptStashReleases: [],
    promptStashState: {},
    agentConnection: {
      prompt: async () => { assert.fail("Setup commands must not require model work"); },
    },
  });

  assert.equal(await mode.runStartupOnboarding(), false);
  assert.equal(shown, true);
  assert.deepEqual(tabs, ["models"]);

  mode.setupEditorSubmitHandler();
  assert.ok(editor.onSubmit);
  await editor.onSubmit("/login");
  await editor.onSubmit("/model");
  // Repeating after another cancellation must still use the local provider menu.
  await editor.onSubmit("/login");
  assert.deepEqual(tabs, ["models", "providers", "models", "providers"]);
});

test("a model-picker refresh is not swallowed by an in-flight cold local catalog read", async () => {
  const mode = Object.create(InteractiveMode.prototype) as {
    getConnectionAvailableModels(options?: { refreshRemote?: boolean }): Promise<unknown[]>;
  };
  const cold = Promise.withResolvers<{ models: []; configuredProviders: [] }>();
  const model = { provider: "deepseek", id: "deepseek-flash" };
  let refreshes = 0;
  Object.assign(mode, {
    connectionModelsRefreshVersion: 0,
    agentConnection: {
      getModelCatalog: () => cold.promise,
      refreshModelCatalog: async () => {
        refreshes++;
        return { models: [model], configuredProviders: ["deepseek"] };
      },
    },
  });
  const reading = mode.getConnectionAvailableModels();
  const picker = mode.getConnectionAvailableModels({ refreshRemote: true });
  cold.resolve({ models: [], configuredProviders: [] });
  assert.deepEqual(await reading, []);
  assert.deepEqual(await picker, [model]);
  assert.equal(refreshes, 1);
});

test("the empty model picker leads to OAuth sign-in and refreshes API-key options", (t) => {
  initTheme("dark");
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager(KEYBINDINGS));
  t.after(() => setKeybindings(previous));
  const selected: unknown[] = [];
  let cancelled = false;
  const provider = { id: "openai-codex", name: "Codex Subscription", authType: "oauth" as const };
  const menu = new ConfigurationMenuComponent({
    initialTab: "models",
    tui: { requestRender: () => {} } as never,
    authStorage: { get: () => undefined, getAuthStatus: () => ({}) } as never,
    modelRegistry: { getProviderAuthStatus: () => ({}) } as never,
    providerOptions: [provider],
    currentModel: undefined,
    availableModels: [],
    scopedModels: [],
    configuredProviders: new Set(),
    requestRender: () => {},
    onSelectProvider: (value) => selected.push(value),
    onSelectModel: () => assert.fail("No model is configured yet"),
    onSelectMcpConnection: () => assert.fail("This is provider setup"),
    onCancel: () => { cancelled = true; },
  });
  assert.match(menu.render(80).join("\n"), /providers/i);
  menu.handleInput("\x0c"); // Ctrl+L: switch from Models to Providers.
  assert.equal(menu.getActiveTab(), "providers");
  assert.match(menu.render(80).join("\n"), /Codex Subscription/);
  menu.handleInput("\r");
  assert.deepEqual(selected, [provider]);
  menu.refreshAuthentication([provider, { id: "deepseek", name: "DeepSeek", authType: "api_key" }]);
  assert.match(menu.render(80).join("\n"), /DeepSeek/);
  menu.handleInput("\x0c");
  assert.equal(menu.getActiveTab(), "models");
  menu.handleInput("\x1b");
  assert.equal(cancelled, true);
});

test("the model picker searches unsigned public models by default even with a saved scope", (t) => {
  initTheme("dark");
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager(KEYBINDINGS));
  t.after(() => setKeybindings(previous));
  const connected = { provider: "openai", id: "gpt-4o", name: "GPT-4o", reasoning: false } as never;
  const publicModel = { provider: "deepseek", id: "deepseek-flash", name: "DeepSeek Flash", reasoning: false } as never;
  const menu = new ConfigurationMenuComponent({
    initialTab: "models",
    tui: { requestRender: () => {} } as never,
    authStorage: {} as never,
    modelRegistry: {} as never,
    providerOptions: [],
    currentModel: connected,
    availableModels: [connected],
    scopedModels: [{ model: connected }],
    configuredProviders: new Set(["openai"]),
    initialModelSearch: "deepseek-flash",
    requestRender: () => {},
    onSelectProvider: () => assert.fail("Expected model selection"),
    onSelectMcpConnection: () => assert.fail("Expected model selection"),
    onSelectModel: (model) => assert.equal(model, publicModel),
    onCancel: () => assert.fail("Expected model selection"),
  });
  const render = () => menu.render(100).join("\n");
  assert.match(render(), /No matching models/);
  menu.updateModels(connected, [connected, publicModel]);
  assert.match(render(), /DeepSeek Flash/);
  assert.match(render(), /require sign in/);
  assert.equal(menu.getSearchValue(), "deepseek-flash");
  menu.handleInput("\x1bs"); // Alt+S: opt into the scoped list.
  assert.match(render(), /No matching models/);
  menu.handleInput("\x1bs");
  assert.match(render(), /DeepSeek Flash/);
  menu.handleInput("\r");
});
