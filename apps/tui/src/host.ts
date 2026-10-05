/**
 * TUI InteractiveMode host.
 * SettingsManager / ModelRegistry / InteractiveMode + Native-backed AgentsViewMode.
 */

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { AuthStorage } from "@earendil-works/pi-coding-agent/auth-storage";
import { ModelRegistry } from "@earendil-works/pi-coding-agent/model-registry";
import { SettingsManager } from "@earendil-works/pi-coding-agent/settings-manager";
import { InteractiveMode } from "@earendil-works/pi-coding-agent/interactive";
import { runAgentsViewMode } from "@earendil-works/pi-coding-agent/agents-view";
import { initTheme } from "@earendil-works/pi-coding-agent/theme";
import type { InteractiveModeUiServices } from "@earendil-works/pi-coding-agent/interactive-services";
import type { NativeAgentConnection } from "./native-agent-connection.js";
import type { AuthCredentialLike } from "./native-connection-ops.js";
import {
  DEVO_APP_TITLE,
  DEVO_COMPACT_ORBIT_LOGO,
  DEVO_SPLASH_TITLE,
  DEVO_TUI_VERSION,
} from "./devo-brand.js";
import { createNativeAgentsViewBackend } from "./native-agents-view-backend.js";

/**
 * Builtins Devo serves itself (or disables) instead of the vendored handler.
 * `/traces` is local-only; `/update` is removed.
 */
export const DEVO_EXCLUDED_BUILTIN_COMMANDS: ReadonlyArray<string> = ["traces", "update"];

export function resolveDevoHome(): string {
  return process.env.DEVO_HOME || path.join(os.homedir(), ".devo");
}

/** Point vendored config/auth at ~/.devo. */
export function applyDevoAgentDirEnv(home = resolveDevoHome()): string {
  fs.mkdirSync(home, { recursive: true });
  // config.ts: ENV_AGENT_DIR follows piConfig.name (devo → DEVO_CODING_AGENT_DIR).
  if (!process.env.DEVO_CODING_AGENT_DIR) {
    process.env.DEVO_CODING_AGENT_DIR = home;
  }
  if (!process.env.DEVO_CODING_AGENT_DIR) {
    process.env.DEVO_CODING_AGENT_DIR = home;
  }
  if (!process.env.DEVO_HOME) {
    process.env.DEVO_HOME = home;
  }
  process.env.PI_SKIP_VERSION_CHECK ??= "1";
  return home;
}

export function createDevoUiServices(options: {
  cwd?: string;
  home?: string;
} = {}): InteractiveModeUiServices & {
  home: string;
  ensureHome: () => string;
  authStorage: AuthStorage;
} {
  const home = options.home ?? resolveDevoHome();
  const cwd = options.cwd ?? process.cwd();
  fs.mkdirSync(home, { recursive: true });

  const authPath = path.join(home, "auth.json");
  const authStorage = AuthStorage.create(authPath, { usePrimeCliConfig: false });
  const modelsJsonPath = path.join(home, "models.json");
  const modelRegistry = ModelRegistry.create(authStorage, modelsJsonPath);
  const settingsManager = SettingsManager.create(cwd, home);

  return {
    settingsManager,
    modelRegistry,
    getInitialCwd: () => cwd,
    getInitialSessionName: () => undefined,
    getThemes: () => [],
    refreshMcpProviders: () => {
      // MCP catalog is server-owned; no-op for first slice.
    },
    home,
    authStorage,
    ensureHome() {
      fs.mkdirSync(home, { recursive: true });
      return home;
    },
  };
}

/**
 * After `/login` stores tokens in AuthStorage, push them to Native
 * via `credential/set`. Wraps AuthStorage.set/remove so later logins sync too.
 */
export function wireAuthCredentialSync(
  connection: NativeAgentConnection,
  authStorage: AuthStorage,
): void {
  const originalSet = authStorage.set.bind(authStorage);
  const originalRemove = authStorage.remove.bind(authStorage);

  authStorage.set = (provider: string, credential: AuthCredentialLike) => {
    originalSet(provider, credential as never);
    void connection.syncAuthStorage(authStorage).catch(() => {});
  };

  authStorage.remove = (provider: string) => {
    originalRemove(provider);
    void (async () => {
      try {
        const listed = await connection.listCredentials();
        for (const c of listed.credentials ?? []) {
          const row = c as { id?: string; provider?: string };
          if (row.provider === provider && row.id) {
            await connection.deleteCredential(row.id);
          }
        }
      } catch {
        // ignore
      }
    })();
  };

  void connection.syncAuthStorage(authStorage).catch(() => {});
}

function createInteractiveMode(
  connection: NativeAgentConnection,
  uiServices: InteractiveModeUiServices,
  options: { forceOnboarding?: boolean; exitAfterOnboarding?: boolean } = {},
): InteractiveMode {
  return new InteractiveMode({
    agentConnection: connection,
    uiServices,
    forceOnboarding: options.forceOnboarding,
    exitAfterOnboarding: options.exitAfterOnboarding,
    bindLocalSessionExtensions: false,
    returnToAgentsView: true,
    preserveAgentConnectionOnHandoff: true,
    forceFullscreen: true,
    agentsViewOwnsStartupNotices: true,
    sessionDepth: 0,
    version: DEVO_TUI_VERSION,
    appTitle: DEVO_APP_TITLE,
    brandSplash: {
      logo: DEVO_COMPACT_ORBIT_LOGO,
      title: DEVO_SPLASH_TITLE,
    },
    excludedLoginProviderIds: ["prime-inference"],
    excludedBuiltinCommands: DEVO_EXCLUDED_BUILTIN_COMMANDS,
  });
}

export async function runInteractiveHost(options: {
  connection: NativeAgentConnection;
  cwd?: string;
  home?: string;
  /** Run onboarding even for an existing configured installation. */
  forceOnboarding?: boolean;
  /** Exit after onboarding instead of opening chat or the agents view. */
  exitAfterOnboarding?: boolean;
}): Promise<{ onboardingCompleted: boolean } | undefined> {
  const home = applyDevoAgentDirEnv(options.home);
  const cwd = options.cwd ?? process.cwd();
  const uiServices = createDevoUiServices({ cwd, home });
  wireAuthCredentialSync(options.connection, uiServices.authStorage);

  // InteractiveMode's constructor calls getEditorTheme() before its own
  // initTheme(); InteractiveMode's main always initializes the theme first.
  initTheme(uiServices.settingsManager.getTheme(), true);

  const nativeBackend = createNativeAgentsViewBackend({
    connection: options.connection,
    cwd,
  });

  // Start in chat; Left enters real AgentsViewMode (Native-backed), which owns
  // the Agents View ↔ chat loop until the user exits the view.
  const interactiveMode = createInteractiveMode(options.connection, uiServices, options);
  const result = await interactiveMode.run();
  if (result.type === "onboarding") {
    return { onboardingCompleted: result.onboardingCompleted };
  }
  if (result.type !== "agents_view" && result.type !== "scoped_agents_view") {
    return;
  }

  const state = await options.connection.getState();
  const sessionId = String(state.sessionId ?? state.activeSessionId ?? "");
  const returnedSummary = sessionId
    ? {
        ...result.source,
        id: sessionId,
        lifecycle: "live" as const,
        activity: "idle" as const,
        isSessionActive: true,
        runtimeKind: "top-level" as const,
        rlmDepth: 0,
        activeSessionId: sessionId,
        sessionId,
        sessionFile: sessionId,
        sessionName: state.sessionName,
        cwd: state.cwd || cwd,
        model: state.model as never,
        thinkingLevel: state.thinkingLevel,
        isStreaming: false,
        isCompacting: false,
        attachedClients: 1,
        messageCount: state.messageCount ?? 0,
        sessionActions: { queuedCount: 0, steering: [], followUps: [] },
      }
    : undefined;

  const initialScopeKey =
    result.type === "scoped_agents_view"
      ? {
          sessionId: result.source.sessionId,
          activeSessionId: result.source.activeSessionId,
        }
      : undefined;

  await runAgentsViewMode({
    nativeBackend,
    config: { cwd, agentDir: home, telemetryDisabled: true },
    uiServices,
    createUiServicesForSession: async () => uiServices,
    verbose: false,
    version: DEVO_TUI_VERSION,
    brandSplash: {
      logo: DEVO_COMPACT_ORBIT_LOGO,
      title: DEVO_SPLASH_TITLE,
    },
    appTitle: DEVO_APP_TITLE,
    excludedLoginProviderIds: ["prime-inference"],
    excludedBuiltinCommands: DEVO_EXCLUDED_BUILTIN_COMMANDS,
    initialSession: returnedSummary,
    initialScopeKey,
  });
}
