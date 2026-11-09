import assert from "node:assert/strict";
import { test } from "node:test";
import { InteractiveMode } from "../lib/coding-agent/src/modes/interactive/interactive-mode.js";
import { ConfigurationMenuComponent } from "../lib/coding-agent/src/modes/interactive/components/configuration-menu.js";
import { initTheme } from "../lib/coding-agent/src/modes/interactive/theme/theme.js";

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

test("provider setup remains selectable with an empty model catalog", () => {
  initTheme("dark");
  const selected: unknown[] = [];
  let cancelled = false;
  const provider = { id: "test-provider", name: "Test provider", authType: "api_key" as const };
  const menu = new ConfigurationMenuComponent({
    initialTab: "providers",
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
  assert.match(menu.render(80).join("\n"), /Test provider/);
  menu.handleInput("\r");
  assert.deepEqual(selected, [provider]);
  menu.handleInput("\x1b");
  assert.equal(cancelled, true);
});
