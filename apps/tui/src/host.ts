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
  // Native owns the model directory. Retain auth helpers without loading legacy
  // models.json or adjacent provider snapshots as a competing catalog.
  const modelRegistry = ModelRegistry.inMemory(authStorage);
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
 * via `credential/set`. Serializes AuthStorage.set; explicit logout awaits and revokes these writes.
 */
const authSyncs = new WeakMap<AuthStorage, { flush(): Promise<void> }>();

export function wireAuthCredentialSync(
  connection: NativeAgentConnection,
  authStorage: AuthStorage,
): void {
  const originalSet = authStorage.set.bind(authStorage);
  let pending = Promise.resolve();
  const enqueue = (operation: () => Promise<unknown>) => {
    pending = pending.catch(() => {}).then(operation).then(() => {});
    // Background OAuth refresh has no awaiting UI caller, but explicit auth
    // flows await flush and surface any failure instead of racing the catalog.
    void pending.catch(() => {});
  };
  authSyncs.set(authStorage, { flush: () => pending });
  authStorage.set = (provider: string, credential: AuthCredentialLike) => {
    originalSet(provider, credential as never);
    if (!provider.startsWith("mcp:")) enqueue(() => connection.setCredentialFromAuth(provider, credential));
  };
  for (const provider of authStorage.list()) {
    const credential = authStorage.get(provider);
    if (credential && (credential.type === "api_key" || credential.type === "oauth") && !provider.startsWith("mcp:")) enqueue(() => connection.setCredentialFromAuth(provider, credential));
  }
}

/** The login intent must be fenced before waiting for prior background writes. */
export async function completeHostProviderAuthentication(
  connection: NativeAgentConnection,
  authStorage: AuthStorage,
  provider: string,
  authType: "oauth" | "api_key",
  kind?: "provider" | "service",
): Promise<void> {
  if (kind !== "provider" || provider.startsWith("mcp:")) {
    await authSyncs.get(authStorage)?.flush();
    return;
  }
  await connection.runExplicitProviderLogin(provider, async (assertCurrent) => {
    await authSyncs.get(authStorage)?.flush();
    assertCurrent();
    const credential = authStorage.get(provider);
    if (authType !== "oauth") {
      if (!credential || credential.type !== "api_key") throw new Error(`Missing API key for ${provider}`);
      await connection.completeExplicitStoredLogin(provider, credential);
      return;
    }
    if (!credential || credential.type !== "oauth") throw new Error(`Missing OAuth credential for ${provider}`);
    await connection.completeExplicitOAuthLogin(provider, credential);
  });
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
    getStoredProviderCredentials: async () => {
      const result = await connection.listCredentials();
      return result.credentials.filter((entry): entry is { provider: string; kind?: string } =>
        typeof (entry as { provider?: unknown }).provider === "string");
    },
    onProviderLogoutStarted: (provider) => {
      if (!provider.startsWith("mcp:")) connection.providerLogoutStarted(provider);
    },
    onProviderLoggedOut: async (provider) => {
      if (provider.startsWith("mcp:")) return;
      // Finish older login writes before deleting; no stale queued sync may
      // recreate the credential after logout has reported success.
      await connection.logoutProvider(provider, authSyncs.get(uiServices.modelRegistry.authStorage)?.flush());
    },
    onProviderAuthenticated: (provider, authType, kind) =>
      completeHostProviderAuthentication(connection, uiServices.modelRegistry.authStorage, provider, authType, kind),
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
