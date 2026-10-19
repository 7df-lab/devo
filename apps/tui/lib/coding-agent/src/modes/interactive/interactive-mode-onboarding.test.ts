import { test } from "node:test";
import assert from "node:assert/strict";
import type { Api, Model } from "@earendil-works/pi-ai";
import { InteractiveMode } from "./interactive-mode.js";

const configuredModel = { provider: "openai", id: "gpt-4o" } as Model<Api>;

type OnboardingHarness = {
  options: { forceOnboarding?: boolean; exitAfterOnboarding?: boolean; onShutdown?: () => void };
  uiServices: {
    settingsManager: {
      getOnboardingShown(): boolean;
      setOnboardingShown(shown: boolean): void;
      flush(): Promise<void>;
    };
    modelRegistry: {
      hasConfiguredAuth(model: Model<Api>): boolean;
      getProviderAuthStatus(provider: string): { source: string };
      authStorage: { get(provider: string): undefined };
      refresh(): void;
    };
  };
  getCurrentModel(): Model<Api> | undefined;
  runOnboardingFlow(): Promise<boolean>;
  runStartupOnboarding(): Promise<boolean>;
  run(): Promise<unknown>;
};

function onboardingHarness(options: OnboardingHarness["options"], selected: boolean, ready = true) {
  const events: string[] = [];
  let shown = true;
  const mode = Object.create(InteractiveMode.prototype) as OnboardingHarness;
  Object.assign(mode, {
    options,
    uiServices: {
      settingsManager: {
        getOnboardingShown: () => shown,
        setOnboardingShown: (value: boolean) => {
          shown = value;
          events.push("shown");
        },
        flush: async () => {
          events.push("flush");
        },
      },
      modelRegistry: {
        hasConfiguredAuth: () => ready,
        getProviderAuthStatus: () => ({ source: "auth.json" }),
        authStorage: { get: () => undefined },
        refresh: () => events.push("refresh"),
      },
    },
    getCurrentModel: () => configuredModel,
    runOnboardingFlow: async () => {
      events.push("onboarding");
      return selected;
    },
  });
  return { mode, events };
}

test("normal startup skips onboarding already shown with a configured model", async () => {
  const { mode, events } = onboardingHarness({}, true);
  assert.equal(await mode.runStartupOnboarding(), false);
  assert.deepEqual(events, []);
});

test("forced onboarding runs when configured and reports success only after model selection", async () => {
  for (const [selected, ready, completed] of [
    [true, true, true],
    [false, true, false],
    [true, false, false],
  ] as const) {
    const { mode, events } = onboardingHarness({ forceOnboarding: true }, selected, ready);
    assert.equal(await mode.runStartupOnboarding(), completed);
    assert.deepEqual(events, ["flush", "onboarding"]);
  }
});

test("exit-after-onboarding tears down the UI before returning and never opens chat", async () => {
  for (const selected of [true, false]) {
    const events: string[] = [];
    const mode = Object.create(InteractiveMode.prototype) as OnboardingHarness & {
      init(): Promise<void>;
      restorePromptStashOnOpen(): void;
      unregisterSignalHandlers(): void;
      teardownSessionUi(): Promise<void>;
      getCurrentCwd(): string;
    };
    Object.assign(mode, {
      options: { exitAfterOnboarding: true, onShutdown: () => events.push("shutdown") },
      init: async () => { events.push("init"); },
      restorePromptStashOnOpen: () => events.push("restore"),
      runStartupOnboarding: async () => { events.push("onboarding"); return selected; },
      unregisterSignalHandlers: () => events.push("unregister"),
      teardownSessionUi: async () => { events.push("teardown"); },
      getCurrentCwd: () => "/tmp",
    });
    assert.deepEqual(await mode.run(), {
      type: "onboarding",
      onboardingCompleted: selected,
      source: { activeSessionId: undefined, sessionFile: undefined, sessionId: "", sessionName: undefined, cwd: "/tmp" },
    });
    assert.deepEqual(events, ["init", "restore", "onboarding", "unregister", "teardown", "shutdown"]);
  }
});

test("onboarding flow distinguishes model selection from cancelling the menu", async () => {
  for (const selected of [true, false]) {
    const events: string[] = [];
    const mode = Object.create(InteractiveMode.prototype) as {
      options: { excludedLoginProviderIds: string[] };
      uiServices: { modelRegistry: { refresh(): void } };
      getModelCandidates(): Promise<unknown[]>;
      showConfigurationMenu(tab: string, search: undefined, onSelected: () => void): Promise<void>;
      runOnboardingFlow(): Promise<boolean>;
    };
    Object.assign(mode, {
      options: { excludedLoginProviderIds: ["prime-inference"] },
      uiServices: { modelRegistry: { refresh: () => events.push("refresh") } },
      getModelCandidates: async () => [configuredModel],
      showConfigurationMenu: async (tab: string, _search: undefined, onSelected: () => void) => {
        events.push(tab);
        if (selected) onSelected();
      },
    });
    assert.equal(await mode.runOnboardingFlow(), selected);
    assert.deepEqual(events, ["refresh", "models"]);
  }
});
